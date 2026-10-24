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
use std::collections::BTreeMap;
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
    extract_tool_calls_with_closed_arguments(text, &BTreeMap::new())
}

/// Enforce authored top-level argument names before JSON maps discard duplicates.
/// Tools absent from `closed_arguments` retain ordinary parsing behavior.
pub fn extract_tool_calls_with_closed_arguments(
    text: &str,
    closed_arguments: &BTreeMap<(String, String), Vec<String>>,
) -> Vec<Result<ParsedFence, FenceParseError>> {
    let mut out = Vec::new();
    let mut cursor = 0usize;
    while let Some(rel) = text[cursor..].find(FENCE_OPEN) {
        let open = cursor + rel;
        let content_start = open + FENCE_OPEN.len();
        match find_fence_close(&text[content_start..]) {
            Some(rel_close) => {
                let content_end = content_start + rel_close;
                let raw = &text[content_start..content_end];
                out.push(parse_fence(open, raw).and_then(|call| {
                    if let Some(allowed) =
                        closed_arguments.get(&(call.server.clone(), call.tool.clone()))
                    {
                        validate_closed_arguments(raw, allowed).map_err(|message| {
                            FenceParseError::InvalidShape {
                                offset: open,
                                message,
                            }
                        })?;
                    }
                    Ok(call)
                }));
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

fn validate_closed_arguments(raw: &str, allowed: &[String]) -> Result<(), String> {
    use serde::de::{IgnoredAny, MapAccess, Visitor};
    use serde::{Deserialize, Deserializer};
    struct Names(Vec<String>);
    impl<'de> Deserialize<'de> for Names {
        fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            struct NamesVisitor;
            impl<'de> Visitor<'de> for NamesVisitor {
                type Value = Names;
                fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                    formatter.write_str("an argument object")
                }
                fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Names, A::Error> {
                    let mut names = Vec::new();
                    while let Some(name) = map.next_key::<String>()? {
                        names.push(name);
                        map.next_value::<IgnoredAny>()?;
                    }
                    Ok(Names(names))
                }
            }
            deserializer.deserialize_map(NamesVisitor)
        }
    }
    #[derive(Deserialize)]
    struct CallNames {
        #[serde(rename = "server")]
        _server: String,
        #[serde(rename = "tool")]
        _tool: String,
        args: Names,
    }
    let names: CallNames = serde_json::from_str(raw).map_err(|error| error.to_string())?;
    let mut seen = std::collections::BTreeSet::new();
    for name in names.args.0 {
        if !seen.insert(name.clone()) {
            return Err(format!("duplicate model argument '{name}'"));
        }
        if !allowed.contains(&name) {
            return Err(format!("model argument '{name}' is not in modelFields"));
        }
    }
    Ok(())
}

// Framing only: serde_json still validates the complete payload. A delimiter
// inside a JSON string is data, including after escaped quotes/backslashes.
fn find_fence_close(text: &str) -> Option<usize> {
    let mut quoted = false;
    let mut escaped = false;
    for (index, byte) in text.bytes().enumerate() {
        if quoted {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                quoted = false;
            }
        } else if byte == b'"' {
            quoted = true;
        } else if byte == b'<' && text[index..].starts_with(FENCE_CLOSE) {
            return Some(index);
        }
    }
    None
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
    tool_call_grammar_with_mode(servers_tools, ToolCallGrammarMode::ProseOrToolCalls)
}

/// Allowed response shapes at the tool-call boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolCallGrammarMode {
    /// Preserve the ordinary coder response: prose or one or more calls.
    ProseOrToolCalls,
    /// Require exactly one call, as required by tool-result completion.
    SingleToolCall,
    /// Require one or more calls, without a prose alternative.
    ToolCallsOnly,
}

/// Build the same granted-tool/JSON grammar with an explicit response shape.
pub fn tool_call_grammar_with_mode(
    servers_tools: &[(String, String)],
    mode: ToolCallGrammarMode,
) -> String {
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
    g.push_str(match mode {
        ToolCallGrammarMode::ProseOrToolCalls => "root ::= ws (fence (ws fence)* | prose) ws\n",
        ToolCallGrammarMode::SingleToolCall => "root ::= ws fence ws\n",
        ToolCallGrammarMode::ToolCallsOnly => "root ::= ws fence (ws fence)* ws\n",
    });
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

/// Restrict model-owned top-level arguments for tools with a projected schema.
/// Primitive fields use their JSON type; the complete wire is validated by
/// the gateway after ledger-owned arguments have been supplied. Other tools
/// and the prose/fence boundary retain the ordinary grammar.
pub fn tool_call_grammar_with_argument_schemas(
    servers_tools: &[(String, String)],
    model_schemas: &BTreeMap<(String, String), Value>,
    mode: ToolCallGrammarMode,
) -> String {
    if model_schemas.is_empty() || servers_tools.is_empty() {
        return tool_call_grammar_with_mode(servers_tools, mode);
    }
    let mut grammar = tool_call_grammar_with_mode(servers_tools, mode)
        .lines()
        .filter(|line| {
            !line.starts_with("call ::=")
                && !line.starts_with("call-s")
                && !line.starts_with("tool-s")
        })
        .map(|line| format!("{line}\n"))
        .collect::<String>();
    let mut pairs = Vec::new();
    for pair in servers_tools {
        if !pairs.contains(pair) {
            pairs.push(pair.clone());
        }
    }
    grammar.push_str(&format!(
        "call ::= {}\n",
        (0..pairs.len())
            .map(|index| format!("call-a{index}"))
            .collect::<Vec<_>>()
            .join(" | ")
    ));
    for (index, (server, tool)) in pairs.iter().enumerate() {
        grammar.push_str(&format!(
            "call-a{index} ::= \"{{\" ws \"\\\"server\\\"\" ws \":\" ws \"{}\" ws \",\" ws \"\\\"tool\\\"\" ws \":\" ws \"{}\" ws \",\" ws \"\\\"args\\\"\" ws \":\" ws args-a{index} \"}}\" ws\n",
            gbnf_escape(&serde_json::to_string(server).expect("JSON string")),
            gbnf_escape(&serde_json::to_string(tool).expect("JSON string")),
        ));
        let Some(schema) = model_schemas
            .get(&(server.clone(), tool.clone()))
            .filter(|schema| schema.get("properties").is_some_and(Value::is_object))
        else {
            grammar.push_str(&format!("args-a{index} ::= object\n"));
            continue;
        };
        let fields = schema["properties"]
            .as_object()
            .expect("projected object schema");
        let required = schema["required"].as_array();
        grammar.push_str(&format!(
            "args-a{index} ::= \"{{\" ws first-a{index}-0 \"}}\" ws\n"
        ));
        // Two suffix states encode optional subsets in canonical key order
        // without exponential alternatives: first has no preceding comma,
        // tail follows an already emitted field. Nested objects remain free.
        for (field_index, name) in fields.keys().enumerate() {
            let next = field_index + 1;
            let key = gbnf_escape(&serde_json::to_string(name).expect("JSON key"));
            let mandatory = required.is_some_and(|names| names.iter().any(|value| value == name));
            // Resolve one local reference only for primitive type selection;
            // patterns, ranges and full schema semantics stay with the gateway.
            let field_schema = fields[name]
                .get("$ref")
                .and_then(Value::as_str)
                .and_then(|reference| reference.strip_prefix('#'))
                .and_then(|pointer| schema.pointer(pointer))
                .unwrap_or(&fields[name]);
            let primitive_rule = match field_schema.get("type").and_then(Value::as_str) {
                Some("string") => "string",
                Some("integer") => "argument-integer",
                Some("number") => "number",
                Some("boolean") => "argument-boolean",
                Some("null") => "argument-null",
                Some("array") => "array",
                Some("object") => "object",
                _ => "value",
            };
            let value_rule = if let Some(values) = field_schema
                .get("enum")
                .and_then(Value::as_array)
                .filter(|values| {
                    !values.is_empty()
                        && values.iter().all(Value::is_string)
                        && field_schema.get("type").and_then(Value::as_str) == Some("string")
                }) {
                let rule = format!("enum-a{index}-{field_index}");
                let alternatives = values
                    .iter()
                    .map(|value| {
                        format!(
                            "\"{}\" ws",
                            gbnf_escape(&serde_json::to_string(value).expect("JSON string enum"))
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(" | ");
                grammar.push_str(&format!("{rule} ::= {alternatives}\n"));
                rule
            } else {
                primitive_rule.to_string()
            };
            for (state, comma) in [("first", ""), ("tail", "\",\" ws ")] {
                grammar.push_str(&format!(
                    "{state}-a{index}-{field_index} ::= {comma}\"{key}\" ws \":\" ws {value_rule} tail-a{index}-{next}{}\n",
                    if mandatory { String::new() } else { format!(" | {state}-a{index}-{next}") },
                ));
            }
        }
        let end = fields.len();
        if schema.get("additionalProperties") == Some(&Value::Bool(false)) {
            grammar.push_str(&format!(
                "first-a{index}-{end} ::= \"\"\ntail-a{index}-{end} ::= \"\"\n"
            ));
        } else {
            // Required known keys precede an unrestricted JSON-member suffix.
            // Open schemas keep arbitrary additional argument names and values.
            grammar.push_str(&format!(
                "first-a{index}-{end} ::= (member (\",\" ws member)*)?\ntail-a{index}-{end} ::= (\",\" ws member)*\n"
            ));
        }
    }
    grammar.push_str("argument-integer ::= \"-\"? (\"0\" | [1-9] [0-9]*) ws\nargument-boolean ::= (\"true\" | \"false\") ws\nargument-null ::= \"null\" ws\n");
    grammar
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
    fn quoted_delimiters_and_escapes_are_payload_data() {
        for value in [
            "<tool_call>",
            "</tool_call>",
            "before \\\" </tool_call> after",
            "backslash \\",
            "<tool_call></tool_call>",
        ] {
            let payload = serde_json::json!({
                "server": "we\"ird\\server", "tool": "t\t工具", "args": {"text": value}
            });
            let text = format!("<tool_call>{payload}</tool_call>");
            let calls = extract_tool_calls(&text);
            assert_eq!(calls.len(), 1, "{text}");
            let parsed = calls[0].as_ref().unwrap();
            assert_eq!(parsed.server, "we\"ird\\server");
            assert_eq!(parsed.tool, "t\t工具");
            assert_eq!(parsed.args["text"], value);
        }
    }

    #[test]
    fn thousands_of_generated_fences_preserve_order_and_escaped_payloads() {
        // Bounded deterministic stress coverage; this is not exhaustive fuzzing.
        let mut text = String::new();
        let mut expected = Vec::new();
        for index in 0..4096 {
            let value = format!(
                "{}{}{}{}",
                "\\".repeat(index % 9),
                if index % 2 == 0 { "\"" } else { "\n\t" },
                "עברית 工具 🦀",
                if index % 3 == 0 {
                    FENCE_OPEN
                } else {
                    FENCE_CLOSE
                },
            );
            let args = serde_json::json!({"index": index, "nested": [{"text": value}]});
            let payload = serde_json::json!({"server": "שרת", "tool": "工具", "args": args});
            text.push_str(&format!("prose 🦀 {FENCE_OPEN}{payload}{FENCE_CLOSE}\n"));
            expected.push(args);
        }
        let calls = extract_tool_calls(&text);
        assert_eq!(calls.len(), expected.len());
        for (call, args) in calls.into_iter().zip(expected) {
            let parsed = call.unwrap();
            assert_eq!(parsed.server, "שרת");
            assert_eq!(parsed.tool, "工具");
            assert_eq!(parsed.args, args);
        }
    }

    #[test]
    fn two_mib_quoted_payload_keeps_delimiters_as_data() {
        let value = "\\\" 🦀 </tool_call> <tool_call>\n".repeat(65_536);
        let payload = serde_json::json!({"server": "s", "tool": "t", "args": {"text": value}});
        let text = format!("{FENCE_OPEN}{payload}{FENCE_CLOSE}");
        assert!(text.len() >= 2 * 1024 * 1024);
        assert!(text.len() < 3 * 1024 * 1024);
        let calls = extract_tool_calls(&text);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].as_ref().unwrap().args["text"], value);
    }

    #[test]
    fn generated_malformed_records_keep_errors_and_following_fences_ordered() {
        let good = r#"{"server":"s","tool":"t","args":{}}"#;
        let malformed = [
            r#"{"server":"s","tool":"t","args":{"text":"工具\q"}}"#,
            r#"{"server":"s","tool":"t","args":{"text":"\uD800"}}"#,
            r#"{"server":"s","tool":"t","args":{"text":"</tool_call>"},}"#,
            r#"{"server":"s","tool":"t","args":{}} 🦀"#,
            r#"{"server":"s","tool":"t","args":[]}"#,
            r#"{"server":null,"tool":"t","args":{}}"#,
        ];
        let mut text = String::new();
        for index in 0..1024 {
            text.push_str(&format!(
                "🦀{FENCE_OPEN}{}{FENCE_CLOSE}{FENCE_OPEN}{good}{FENCE_CLOSE}",
                malformed[index % malformed.len()]
            ));
        }
        let calls = extract_tool_calls(&text);
        assert_eq!(calls.len(), 2048);
        for pair in calls.chunks_exact(2) {
            assert!(pair[0].is_err());
            assert!(pair[1].is_ok());
        }
        // Unterminated strings hide closing tags as data, yielding one precise
        // byte-offset error rather than treating the quoted delimiter as framing.
        for suffix in [
            r#"{"args":{"text":"🦀</tool_call>"#,
            r#"{"args":{"text":"\\"#,
        ] {
            let text = format!("🦀{FENCE_OPEN}{suffix}");
            assert!(matches!(extract_tool_calls(&text).as_slice(),
                [Err(FenceParseError::UnclosedFence { offset })] if *offset == "🦀".len()));
        }
    }

    #[test]
    fn framing_does_not_accept_invalid_or_trailing_json() {
        for payload in [
            r#"{"server":"s","tool":"t","args":{"text":"</tool_call>"},}"#,
            r#"{"server":"s","tool":"t","args":{}} trailing"#,
            r#"{"server":"s","tool":"t","args":{"text":"bad\q"}}"#,
        ] {
            let text = format!("<tool_call>{payload}</tool_call><tool_call>{{\"server\":\"s\",\"tool\":\"t\",\"args\":{{}}}}</tool_call>");
            let calls = extract_tool_calls(&text);
            assert_eq!(calls.len(), 2, "{text}");
            assert!(matches!(calls[0], Err(FenceParseError::InvalidJson { .. })));
            assert!(calls[1].is_ok());
        }
        let calls = extract_tool_calls(r#"<tool_call>{"args":{"text":"</tool_call>"}}"#);
        assert!(matches!(
            calls.as_slice(),
            [Err(FenceParseError::UnclosedFence { .. })]
        ));
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
        // execute GBNF in default tests, so these are structural
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

#[cfg(test)]
mod argument_schema_grammar_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn string_enum_values_are_json_and_gbnf_escaped_and_have_no_free_string_branch() {
        let values = ["finish", "quote\"slash\\line\n", "שלום"];
        let schemas = BTreeMap::from([(
            ("flow".into(), "choose".into()),
            json!({"type":"object", "properties":{"route":{"type":"string","enum":values}}, "required":["route"],"additionalProperties":false}),
        )]);
        let grammar = tool_call_grammar_with_argument_schemas(
            &[("flow".into(), "choose".into())],
            &schemas,
            ToolCallGrammarMode::SingleToolCall,
        );
        let rule = grammar
            .lines()
            .find(|line| line.starts_with("enum-a0-0 ::="))
            .unwrap();
        for value in values {
            assert!(rule.contains(&gbnf_escape(&serde_json::to_string(value).unwrap())));
        }
        assert!(!rule.contains(" | string"));
        assert!(grammar.contains("ws enum-a0-0 tail-a0-1"));
    }

    #[test]
    fn closed_arguments_reject_raw_duplicates_while_open_calls_remain_compatible() {
        let closed = BTreeMap::from([(("t".into(), "read".into()), vec!["value".into()])]);
        for raw in [
            r#"{"server":"t","tool":"read","args":{"value":1,"value":2}}"#,
            r#"{"server":"t","tool":"read","args":{"value":1,"\u0076alue":2}}"#,
            r#"{"server":"t","server":"t","tool":"read","args":{}}"#,
            r#"{"server":"t","tool":"read","tool":"read","args":{}}"#,
            r#"{"server":"t","tool":"read","args":{"unknown":1}}"#,
        ] {
            let text = format!("{FENCE_OPEN}{raw}{FENCE_CLOSE}");
            assert!(extract_tool_calls_with_closed_arguments(&text, &closed)[0].is_err());
            assert!(extract_tool_calls(&text)[0].is_ok());
        }
        let text =
            r#"<tool_call>{"server":"t","tool":"read","args":{"value":{"nested":1}}}</tool_call>"#;
        assert!(extract_tool_calls_with_closed_arguments(text, &closed)[0].is_ok());
    }

    #[test]
    fn closed_argument_validation_is_scoped_and_keeps_scanning_after_errors() {
        let closed = BTreeMap::from([(("t".into(), "closed".into()), vec!["value".into()])]);
        let text = concat!(
            r#"<tool_call>{"server":"t","tool":"closed","args":{},"args":{"value":1}}</tool_call>"#,
            r#"<tool_call>{"server":"t","tool":"open","args":{"unknown":1,"unknown":2}}</tool_call>"#,
            r#"<tool_call>{"server":"t","tool":"closed","args":{"\u0076alue":{"nested":1,"nested":2}}}</tool_call>"#,
        );
        let calls = extract_tool_calls_with_closed_arguments(text, &closed);
        assert_eq!(calls.len(), 3);
        assert!(
            calls[0].is_err(),
            "duplicate args envelopes must be rejected"
        );
        assert_eq!(calls[1].as_ref().unwrap().args["unknown"], 2);
        assert_eq!(calls[2].as_ref().unwrap().args["value"]["nested"], 2);
    }

    #[test]
    fn closed_optional_fields_have_no_generic_member_fallback() {
        let pairs = vec![("t".into(), "read".into())];
        let schemas = BTreeMap::from([(
            ("t".into(), "read".into()),
            json!({"type":"object","properties":{"cursor":{"type":"string"}},"additionalProperties":false}),
        )]);
        let grammar = tool_call_grammar_with_argument_schemas(
            &pairs,
            &schemas,
            ToolCallGrammarMode::SingleToolCall,
        );
        assert!(grammar.contains("first-a0-1 ::= \"\"\ntail-a0-1 ::= \"\""));
        assert!(!grammar
            .lines()
            .filter(|line| line.starts_with("first-a0-") || line.starts_with("tail-a0-"))
            .any(|line| line.contains("member")));
    }

    #[test]
    fn required_keys_use_typed_rules_and_bound_empty_object_remains_legal() {
        for field in ["nodeId", "text", "left", "route"] {
            let pairs = vec![("generic".into(), "operation".into())];
            let schemas = BTreeMap::from([(
                ("generic".into(), "operation".into()),
                json!({
                    "type":"object", "properties":{field:{"type":"string"}}, "required":[field], "additionalProperties":false
                }),
            )]);
            let grammar = tool_call_grammar_with_argument_schemas(
                &pairs,
                &schemas,
                ToolCallGrammarMode::SingleToolCall,
            );
            let field_rule = grammar
                .lines()
                .find(|line| line.starts_with("first-a0-0 ::="))
                .unwrap();
            assert!(field_rule.contains("ws string tail-a0-1"));
            assert!(!field_rule.contains(" | "));
            assert!(!field_rule.contains("node_id"));
            let empty = BTreeMap::from([(
                ("generic".into(), "operation".into()),
                json!({"type":"object","properties":{},"required":[],"additionalProperties":false}),
            )]);
            let grammar = tool_call_grammar_with_argument_schemas(
                &pairs,
                &empty,
                ToolCallGrammarMode::SingleToolCall,
            );
            assert!(grammar.contains("first-a0-0 ::= \"\""));
        }
    }
}
