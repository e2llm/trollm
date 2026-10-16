//! The tool-call fence (docs/ARCHITECTURE-ANDROID.md §5.1): the ONLY structured
//! model output in Trollm.
//!
//! ```text
//! <tool_call>{"server":"tracker","tool":"get_issue","args":{"id":123}}</tool_call>
//! ```
//!
//! This module provides:
//! - [`tool_call_grammar`]: a GBNF grammar (llama.cpp format) constraining a
//!   generation to one or more fences — or to prose that cannot contain the
//!   fence-open token (docs/ARCHITECTURE-ANDROID.md §14 item 5) — with
//!   `server`/`tool` closed over the supplied allowlist and `args` constrained
//!   to a syntactically valid JSON object (arg-level validation is post-hoc,
//!   per §5.1).
//! - [`extract_tool_calls`]: a tolerant fence parser. Everything outside fences
//!   is prose and ignored; multiple fences per generation are returned in order.

use serde_json::Value;
use std::fmt;

/// Opening tag of the tool-call fence.
pub const FENCE_OPEN: &str = "<tool_call>";
/// Closing tag of the tool-call fence.
pub const FENCE_CLOSE: &str = "</tool_call>";

/// A successfully parsed tool-call fence.
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedFence {
    pub server: String,
    pub tool: String,
    /// The `args` object, exactly as parsed. Schema-level validation against the
    /// tool's input schema is the caller's job (§5.1: validated post-hoc).
    pub args: Value,
}

/// Why a single fence failed to parse. Extraction continues past failed fences.
#[derive(Debug, Clone, PartialEq)]
pub enum FenceParseError {
    /// `<tool_call>` with no matching `</tool_call>` before end of text.
    UnclosedFence {
        /// Byte offset of the opening tag.
        offset: usize,
    },
    /// Fence content is not valid JSON.
    InvalidJson {
        /// Byte offset of the opening tag.
        offset: usize,
        message: String,
    },
    /// Valid JSON, but not `{"server": string, "tool": string, "args": object}`.
    InvalidShape {
        /// Byte offset of the opening tag.
        offset: usize,
        message: String,
    },
}

impl fmt::Display for FenceParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FenceParseError::UnclosedFence { offset } => {
                write!(f, "unclosed tool-call fence at byte {offset}")
            }
            FenceParseError::InvalidJson { offset, message } => {
                write!(
                    f,
                    "invalid JSON in tool-call fence at byte {offset}: {message}"
                )
            }
            FenceParseError::InvalidShape { offset, message } => {
                write!(f, "malformed tool-call fence at byte {offset}: {message}")
            }
        }
    }
}

impl std::error::Error for FenceParseError {}

/// Extract every tool-call fence from `text`, in order of appearance.
///
/// Prose before, between, and after fences is ignored (that is the point of the
/// fence). Each fence yields `Ok(ParsedFence)` or its own `Err(FenceParseError)`;
/// a malformed fence does not abort the scan of the remaining text. Text with no
/// fences yields an empty vec.
pub fn extract_tool_calls(text: &str) -> Vec<Result<ParsedFence, FenceParseError>> {
    let mut out = Vec::new();
    let mut cursor = 0usize;
    while let Some(rel) = text[cursor..].find(FENCE_OPEN) {
        let open = cursor + rel;
        let content_start = open + FENCE_OPEN.len();
        match text[content_start..].find(FENCE_CLOSE) {
            Some(rel_close) => {
                let content_end = content_start + rel_close;
                out.push(parse_fence(open, &text[content_start..content_end]));
                cursor = content_end + FENCE_CLOSE.len();
            }
            None => {
                out.push(Err(FenceParseError::UnclosedFence { offset: open }));
                break;
            }
        }
    }
    out
}

fn parse_fence(offset: usize, raw: &str) -> Result<ParsedFence, FenceParseError> {
    let value: Value =
        serde_json::from_str(raw.trim()).map_err(|e| FenceParseError::InvalidJson {
            offset,
            message: e.to_string(),
        })?;
    let obj = value
        .as_object()
        .ok_or_else(|| FenceParseError::InvalidShape {
            offset,
            message: "fence content must be a JSON object".into(),
        })?;
    let server =
        obj.get("server")
            .and_then(Value::as_str)
            .ok_or_else(|| FenceParseError::InvalidShape {
                offset,
                message: "\"server\" must be a string".into(),
            })?;
    let tool =
        obj.get("tool")
            .and_then(Value::as_str)
            .ok_or_else(|| FenceParseError::InvalidShape {
                offset,
                message: "\"tool\" must be a string".into(),
            })?;
    let args =
        obj.get("args")
            .filter(|v| v.is_object())
            .ok_or_else(|| FenceParseError::InvalidShape {
                offset,
                message: "\"args\" must be an object".into(),
            })?;
    Ok(ParsedFence {
        server: server.to_string(),
        tool: tool.to_string(),
        args: args.clone(),
    })
}

/// Build a GBNF grammar (llama.cpp format) constraining a generation to one or
/// more tool-call fences whose `server`/`tool` pairs come from `servers_tools`
/// (`(server, tool)` entries; a server may appear several times, once per tool).
///
/// Per §5.1, `args` is constrained only to be a syntactically valid JSON object
/// (generic object grammar); argument correctness is validated post-hoc against
/// the tool's input schema.
///
/// Grammar root (docs/ARCHITECTURE-ANDROID.md §14 item 5): `ws (fence+ | prose)`.
/// A constrained coder generation either emits only well-formed fences, or
/// emits prose that cannot contain the fence-open token `<tool_call>` — so
/// prose loop-termination stays possible on constrained decoding steps. The
/// grammar is meant to be applied to every coder-slot generation.
///
/// The prose constraint is exact, not a fallback: since the forbidden token is
/// a fixed literal, a per-position suffix chain (`fnc-*` rules) forbids
/// completing `<tool_call>` inside prose while allowing every other byte,
/// including partial prefixes like `<tool_call` mid-text and nested `<`
/// restarts (`<<tool_call>` is still rejected). The one over-strict corner:
/// prose cannot *end* with an unterminated `<tool_call` prefix (GBNF has no
/// end-of-input lookahead); such text also does not contain a complete fence,
/// so nothing legal is lost.
///
/// Degenerate case: an empty allowlist constrains shape only — `server`/`tool`
/// fall back to arbitrary JSON strings (there are no legal tool calls anyway;
/// the ledger should not be asking for one).
pub fn tool_call_grammar(servers_tools: &[(String, String)]) -> String {
    // Group tools by server, preserving first-appearance order.
    let mut servers: Vec<(&str, Vec<&str>)> = Vec::new();
    for (server, tool) in servers_tools {
        match servers.iter_mut().find(|(s, _)| *s == server.as_str()) {
            Some((_, tools)) => {
                if !tools.contains(&tool.as_str()) {
                    tools.push(tool.as_str());
                }
            }
            None => servers.push((server.as_str(), vec![tool.as_str()])),
        }
    }

    let mut g = String::new();
    g.push_str("# Tool-call fence grammar (docs/ARCHITECTURE-ANDROID.md §5.1).\n");
    g.push_str("# Generated by trollm_models::fence::tool_call_grammar.\n");
    g.push_str("root ::= ws (fence (ws fence)* | prose) ws\n");
    g.push_str(&format!(
        "fence ::= \"{}\" ws call ws \"{}\"\n",
        gbnf_escape(FENCE_OPEN),
        gbnf_escape(FENCE_CLOSE)
    ));

    if servers.is_empty() {
        g.push_str("call ::= \"{\" ws \"\\\"server\\\"\" ws \":\" ws string \",\" ws \"\\\"tool\\\"\" ws \":\" ws string \",\" ws \"\\\"args\\\"\" ws \":\" ws object \"}\" ws\n");
    } else {
        let call_alts: Vec<String> = (0..servers.len()).map(|i| format!("call-s{i}")).collect();
        g.push_str(&format!("call ::= {}\n", call_alts.join(" | ")));
        for (i, (server, tools)) in servers.iter().enumerate() {
            g.push_str(&format!(
                "call-s{i} ::= \"{{\" ws \"\\\"server\\\"\" ws \":\" ws \"{}\" ws \",\" ws \"\\\"tool\\\"\" ws \":\" ws tool-s{i} ws \",\" ws \"\\\"args\\\"\" ws \":\" ws object \"}}\" ws\n",
                gbnf_escape(&serde_json::to_string(server).expect("JSON string"))
            ));
            let tool_alts: Vec<String> = tools
                .iter()
                .map(|t| {
                    format!(
                        "\"{}\"",
                        gbnf_escape(&serde_json::to_string(t).expect("JSON string"))
                    )
                })
                .collect();
            g.push_str(&format!("tool-s{i} ::= {}\n", tool_alts.join(" | ")));
        }
    }

    // Generic JSON rules for `args` (shape-only, per §5.1).
    g.push_str(
        "object ::= \"{\" ws (member (\",\" ws member)*)? \"}\" ws\n\
         member ::= string \":\" ws value\n\
         value ::= object | array | string | number | \"true\" ws | \"false\" ws | \"null\" ws\n\
         array ::= \"[\" ws (value (\",\" ws value)*)? \"]\" ws\n\
         string ::= \"\\\"\" string-chars \"\\\"\" ws\n\
         string-chars ::= ([^\"\\\\\\x00-\\x1F] | \"\\\\\" escape)*\n\
         escape ::= [\"\\\\/bfnrt] | \"u\" hex hex hex hex\n\
         hex ::= [0-9a-fA-F]\n\
         number ::= \"-\"? (\"0\" | [1-9] [0-9]*) (\".\" [0-9]+)? ([eE] [-+]? [0-9]+)? ws\n\
         ws ::= [ \\t\\n\\r]*\n",
    );

    // Prose alternative (§14 item 5): any text that never completes the
    // fence-open token. `fnc-k` = "we have matched the first k chars of
    // FENCE_OPEN[1..] after a `<`; the next char must not extend the match,
    // and a fresh `<` restarts the check". The last rule has no completion
    // alternative, so the full token can never appear inside prose.
    let tail: Vec<char> = FENCE_OPEN[1..].chars().collect();
    debug_assert!(
        tail.iter()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '>')),
        "fence-open tail must stay char-class-safe"
    );
    g.push_str("prose ::= prose-item+\n");
    g.push_str("prose-item ::= [^<] | \"<\" fnc-0\n");
    for (k, &c) in tail.iter().enumerate() {
        let mut rule = format!("fnc-{k} ::= [^{c}<] | \"<\" fnc-0");
        if k + 1 < tail.len() {
            rule.push_str(&format!(" | \"{c}\" fnc-{}", k + 1));
        }
        g.push_str(&rule);
        g.push('\n');
    }
    g
}

/// Escape a string for inclusion as a GBNF double-quoted literal.
fn gbnf_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\x{:02X}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prose_only_yields_empty() {
        assert!(extract_tool_calls("close this ticket, it is stale.").is_empty());
        assert!(extract_tool_calls("").is_empty());
    }

    #[test]
    fn single_fence_with_surrounding_prose() {
        let text = "Let me look that up.\n<tool_call>{\"server\":\"tracker\",\"tool\":\"get_issue\",\"args\":{\"id\":123}}</tool_call>\n";
        let calls = extract_tool_calls(text);
        assert_eq!(calls.len(), 1);
        let parsed = calls[0].as_ref().expect("fence should parse");
        assert_eq!(parsed.server, "tracker");
        assert_eq!(parsed.tool, "get_issue");
        assert_eq!(parsed.args, serde_json::json!({"id": 123}));
    }

    #[test]
    fn multiple_fences_in_order() {
        let text = concat!(
            "<tool_call>{\"server\":\"tracker\",\"tool\":\"get_issue\",\"args\":{\"id\":1}}</tool_call>",
            " and then ",
            "<tool_call>{\"server\":\"files\",\"tool\":\"read\",\"args\":{\"path\":\"/tmp/x\"}}</tool_call>",
            " done."
        );
        let calls = extract_tool_calls(text);
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].as_ref().unwrap().tool, "get_issue");
        assert_eq!(calls[1].as_ref().unwrap().server, "files");
        assert_eq!(calls[1].as_ref().unwrap().args["path"], "/tmp/x");
    }

    #[test]
    fn malformed_json_inside_fence_yields_err_and_scan_continues() {
        let text = concat!(
            "prose ",
            "<tool_call>{\"server\":\"tracker\",\"tool\":\"get_issue\",\"args\":</tool_call>",
            " more prose ",
            "<tool_call>{\"server\":\"tracker\",\"tool\":\"list_issues\",\"args\":{}}</tool_call>"
        );
        let calls = extract_tool_calls(text);
        assert_eq!(calls.len(), 2);
        assert!(matches!(calls[0], Err(FenceParseError::InvalidJson { .. })));
        assert_eq!(calls[1].as_ref().unwrap().tool, "list_issues");
    }

    #[test]
    fn wrong_shape_yields_err() {
        // args must be an object; server/tool must be strings.
        let calls = extract_tool_calls(
            "<tool_call>{\"server\":\"tracker\",\"tool\":\"get_issue\",\"args\":[1]}</tool_call>",
        );
        assert_eq!(calls.len(), 1);
        assert!(matches!(
            calls[0],
            Err(FenceParseError::InvalidShape { .. })
        ));

        let calls =
            extract_tool_calls("<tool_call>{\"server\":\"tracker\",\"args\":{}}</tool_call>");
        assert!(matches!(
            calls[0],
            Err(FenceParseError::InvalidShape { .. })
        ));
    }

    #[test]
    fn unclosed_fence_yields_err() {
        let calls = extract_tool_calls("text <tool_call>{\"server\":\"tracker\"");
        assert_eq!(calls.len(), 1);
        assert!(matches!(
            calls[0],
            Err(FenceParseError::UnclosedFence { .. })
        ));
    }

    #[test]
    fn fence_spanning_lines() {
        let text = "I'll fetch it:\n<tool_call>{\n  \"server\": \"tracker\",\n  \"tool\": \"get_issue\",\n  \"args\": {\n    \"id\": 42\n  }\n}\n</tool_call>\nDone.";
        let calls = extract_tool_calls(text);
        assert_eq!(calls.len(), 1);
        let parsed = calls[0].as_ref().unwrap();
        assert_eq!(parsed.args["id"], 42);
    }

    #[test]
    fn grammar_has_root_and_enumerates_pairs() {
        // NOTE: we cannot execute GBNF without llama.cpp (the `llama` feature is
        // off in tests), so these are structural assertions on the grammar text
        // only — nothing is "rejected" here.
        let grammar = tool_call_grammar(&[
            ("tracker".to_string(), "get_issue".to_string()),
            ("tracker".to_string(), "list_issues".to_string()),
            ("files".to_string(), "read".to_string()),
        ]);
        assert!(
            grammar.contains("root ::="),
            "grammar must define a root rule"
        );
        assert!(
            grammar.contains("fence ::="),
            "grammar must define the fence"
        );
        assert!(
            grammar.contains(FENCE_OPEN),
            "grammar must emit the open tag"
        );
        assert!(
            grammar.contains(FENCE_CLOSE),
            "grammar must emit the close tag"
        );
        // Server/tool names enumerated, tool scoped per server.
        assert!(grammar.contains(&gbnf_escape("\"tracker\"")));
        assert!(grammar.contains(&gbnf_escape("\"files\"")));
        assert!(grammar.contains(&gbnf_escape("\"get_issue\"")));
        assert!(grammar.contains(&gbnf_escape("\"list_issues\"")));
        assert!(grammar.contains(&gbnf_escape("\"read\"")));
        // args constrained to a generic JSON object.
        assert!(grammar.contains("object ::="));
        assert!(grammar.contains("value ::="));
        // Duplicate pairs must not duplicate rules.
        let grammar2 = tool_call_grammar(&[
            ("tracker".to_string(), "get_issue".to_string()),
            ("tracker".to_string(), "get_issue".to_string()),
        ]);
        assert_eq!(grammar2.matches("get_issue").count(), 1);
    }

    #[test]
    fn grammar_escapes_gbnf_specials() {
        let grammar = tool_call_grammar(&[("we\"ird\\server".to_string(), "t\ttool".to_string())]);
        let server_json = serde_json::to_string("we\"ird\\server").unwrap();
        let tool_json = serde_json::to_string("t\ttool").unwrap();
        assert!(grammar.contains(&format!("\"{}\"", gbnf_escape(&server_json))));
        assert!(grammar.contains(&format!("\"{}\"", gbnf_escape(&tool_json))));
    }

    #[test]
    fn empty_allowlist_still_yields_valid_shape() {
        let grammar = tool_call_grammar(&[]);
        assert!(grammar.contains("root ::="));
        assert!(grammar.contains("call ::="));
    }

    #[test]
    fn grammar_root_is_fences_or_prose() {
        // §14 item 5: root = ws (fence+ | prose). NOTE: as above, we cannot
        // execute GBNF without llama.cpp + weights, so these are structural
        // assertions on the grammar text only.
        let grammar = tool_call_grammar(&[("tracker".to_string(), "get_issue".to_string())]);
        assert!(
            grammar.contains("root ::= ws (fence (ws fence)* | prose) ws"),
            "root must offer both the fence sequence and the prose alternative"
        );
        assert!(grammar.contains("prose ::= prose-item+"));
        assert!(grammar.contains("prose-item ::= [^<] | \"<\" fnc-0"));
    }

    #[test]
    fn grammar_prose_forbids_fence_open_token() {
        // The prose path must make `<tool_call>` unemittable. The generated
        // `fnc-k` chain walks the fence-open tail "tool_call>" one char at a
        // time; every rule offers a bail-out char class and a `<` restart, and
        // the last rule (expected char `>`) offers NO completion alternative.
        let grammar = tool_call_grammar(&[("tracker".to_string(), "get_issue".to_string())]);
        let tail: Vec<char> = FENCE_OPEN[1..].chars().collect(); // "tool_call>"
        assert_eq!(tail.len(), 10, "test assumes fence-open tail of length 10");
        for (k, &c) in tail.iter().enumerate() {
            let rule = grammar
                .lines()
                .find(|l| l.starts_with(&format!("fnc-{k} ::=")))
                .unwrap_or_else(|| panic!("missing rule fnc-{k}"));
            assert!(
                rule.contains(&format!("[^{c}<]")),
                "fnc-{k} must bail out on any char other than '{c}' or '<'"
            );
            assert!(
                rule.contains("\"<\" fnc-0"),
                "fnc-{k} must restart the check on a fresh '<'"
            );
            if k + 1 < tail.len() {
                assert!(
                    rule.contains(&format!("\"{c}\" fnc-{}", k + 1)),
                    "fnc-{k} must advance on '{c}'"
                );
            } else {
                // The final rule must not allow emitting the closing '>' that
                // would complete the fence-open token inside prose.
                assert!(
                    !rule.contains(&format!("\"{c}\" fnc-{}", k + 1)),
                    "fnc-{k} must not complete the fence-open token"
                );
            }
        }
    }

    #[test]
    fn allowlisted_gbnf_terminals_derive_json_strings() {
        // Decode GBNF's quoted terminals independently through JSON escaping,
        // then parse the resulting fence. Names with quotes, slashes, control
        // characters and Unicode must survive both quoting layers.
        fn terminals(rule: &str) -> Vec<String> {
            let bytes = rule.as_bytes();
            let mut result = Vec::new();
            let mut index = 0;
            while index < bytes.len() {
                if bytes[index] != b'"' {
                    index += 1;
                    continue;
                }
                let start = index;
                index += 1;
                while index < bytes.len() {
                    if bytes[index] == b'\\' {
                        index += 2;
                    } else if bytes[index] == b'"' {
                        index += 1;
                        break;
                    } else {
                        index += 1;
                    }
                }
                result.push(serde_json::from_str(&rule[start..index]).unwrap());
            }
            result
        }
        for (server, tool) in [
            ("tracker", "read"),
            ("we\"ird\\server", "t\ttool"),
            ("שרת", "工具"),
        ] {
            let grammar = tool_call_grammar(&[(server.into(), tool.into())]);
            let server_rule = grammar
                .lines()
                .find(|line| line.starts_with("call-s0 ::="))
                .unwrap();
            let tool_rule = grammar
                .lines()
                .find(|line| line.starts_with("tool-s0 ::="))
                .unwrap();
            let server_literal = &terminals(server_rule)[3];
            let tool_literal = &terminals(tool_rule)[0];
            let fence = format!("<tool_call>{{\"server\":{server_literal},\"tool\":{tool_literal},\"args\":{{\"nested\":{{\"id\":7}}}}}}</tool_call>");
            let parsed = extract_tool_calls(&fence).remove(0).unwrap();
            assert_eq!(parsed.server, server);
            assert_eq!(parsed.tool, tool);
            assert_eq!(parsed.args["nested"]["id"], 7);
        }
    }
}
