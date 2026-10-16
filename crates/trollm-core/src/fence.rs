//! Extraction of `<tool_call>...</tool_call>` fences (§5.1) — the ONLY
//! structured model output. Everything outside fences is prose. Multiple fences
//! per generation are allowed and executed in order; a generation with no fence
//! terminates the coder loop with prose output.

use trollm_mcp::ToolCall;

#[derive(Debug, thiserror::Error)]
#[error("tool-call-parse: {0}")]
pub struct FenceError(pub String);

/// Pull every fence out of a generation. Returns (fences, had_fence).
/// Any malformed fence is a `tool-call-parse` error (§5.1, §4.5 error classes).
pub fn extract_tool_calls(text: &str) -> Result<Vec<ToolCall>, FenceError> {
    trollm_models::fence::extract_tool_calls(text)
        .into_iter()
        .map(|parsed| {
            parsed
                .map(|call| ToolCall {
                    server: call.server,
                    tool: call.tool,
                    args: call.args,
                })
                .map_err(|error| FenceError(error.to_string()))
        })
        .collect()
}

/// Prose = everything outside the fences (§5.1), whitespace-collapsed at edges.
pub fn prose_outside_fences(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find("<tool_call>") {
        out.push_str(&rest[..start]);
        let after = &rest[start + "<tool_call>".len()..];
        match after.find("</tool_call>") {
            Some(end) => rest = &after[end + "</tool_call>".len()..],
            None => {
                rest = "";
                break;
            }
        }
    }
    out.push_str(rest);
    out.trim().to_string()
}
