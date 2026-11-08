//! Bounded, executor-associated preparation before mutation intent.
//! These records do not authorize a send or settle an uncertain effect.
use crate::{CallContext, ToolCall};
use anyhow::{ensure, Result};
use serde::ser::{SerializeMap, SerializeSeq};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const MAX_PREPARATION_BYTES: usize = 65_536;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FramePosition {
    pub plan: String,
    pub node: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CursorPosition {
    pub node: String,
    pub snapshot_ref: String,
    pub index: u64,
    pub phase: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BoundArgument {
    pub name: String,
    pub source: String,
    pub path: Option<String>,
    pub value: serde_json::Value,
}

/// Supplied by the executor, never deserialized from model arguments.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreparationContext {
    pub run_id: String,
    pub node: String,
    pub task_ref: String,
    pub model_slot: String,
    pub model_call_ordinal: u32,
    pub frames: Vec<FramePosition>,
    pub cursors: Vec<CursorPosition>,
    pub argument_bindings: Vec<BoundArgument>,
    pub call_context: CallContext,
}

/// The inner JSON is an opaque string across JavaScript. In particular its
/// native u64 attempt must not pass through a JavaScript numeric decoder.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RawContext {
    pub json: String,
    pub sha256: String,
    pub call_context: LexicalCallContext,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LexicalCallContext {
    pub epoch: String,
    pub attempt: String,
    pub call: u32,
}

pub fn raw_sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// The supplier/private-IPC contracts use JavaScript key order and safe integer
/// JSON. This is serialization only, never a profiled logical digest.
pub fn js_sorted_json_bytes(value: &serde_json::Value) -> Result<Vec<u8>> {
    struct Canonical<'a>(&'a serde_json::Value);
    impl Serialize for Canonical<'_> {
        fn serialize<S: serde::Serializer>(
            &self,
            serializer: S,
        ) -> std::result::Result<S::Ok, S::Error> {
            match self.0 {
                serde_json::Value::Object(fields) => {
                    let mut keys: Vec<_> = fields.keys().collect();
                    keys.sort_by(|a, b| a.encode_utf16().cmp(b.encode_utf16()));
                    let mut map = serializer.serialize_map(Some(keys.len()))?;
                    for key in keys {
                        map.serialize_entry(key, &Canonical(&fields[key]))?;
                    }
                    map.end()
                }
                serde_json::Value::Array(values) => {
                    let mut seq = serializer.serialize_seq(Some(values.len()))?;
                    for value in values {
                        seq.serialize_element(&Canonical(value))?;
                    }
                    seq.end()
                }
                serde_json::Value::Number(number) => {
                    let safe = number.as_u64().is_some_and(|n| n <= 9_007_199_254_740_991)
                        || number
                            .as_i64()
                            .is_some_and(|n| n.unsigned_abs() <= 9_007_199_254_740_991);
                    if !safe {
                        return Err(serde::ser::Error::custom(
                            "integer JSON requires lexical unsafe quantities",
                        ));
                    }
                    self.0.serialize(serializer)
                }
                value => value.serialize(serializer),
            }
        }
    }
    Ok(serde_json::to_vec(&Canonical(value))?)
}

impl PreparationContext {
    pub fn raw(&self) -> Result<RawContext> {
        let json = serde_json::to_string(self)?;
        ensure!(
            json.len() <= MAX_PREPARATION_BYTES,
            "native preparation context exceeds limit"
        );
        Ok(RawContext {
            sha256: raw_sha256(json.as_bytes()),
            json,
            call_context: LexicalCallContext {
                epoch: self.call_context.epoch.clone(),
                attempt: self.call_context.attempt.to_string(),
                call: self.call_context.call,
            },
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PreparationEvidence {
    pub provider: String,
    pub context_sha256: String,
    /// Exact once-serialized adapter response, not original Relay reply bytes.
    pub response_json: String,
    pub response_sha256: String,
}

#[derive(Clone)]
pub struct PreparedCall {
    pub call: ToolCall,
    pub evidence: PreparationEvidence,
}

/// Installed by the native owner, never constructed by a model or task argument.
/// A provider may perform bounded read-only I/O during preparation. The final
/// check runs with the native client lock held and after credential lookup.
pub trait CallPreparer: Send + Sync {
    fn prepared_argument_names(&self, _server: &str, _tool: &str) -> Vec<String> {
        Vec::new()
    }
    fn after_retained_intent(
        &self,
        _resolved: &ToolCall,
        _context: &PreparationContext,
        _evidence: &PreparationEvidence,
        _intent_seq: u64,
    ) -> Result<()> {
        anyhow::bail!("selected retained-intent verification unavailable")
    }
    fn prepare(
        &self,
        requested: &ToolCall,
        context: &PreparationContext,
        deadline: Option<std::time::Instant>,
    ) -> Result<PreparedCall>;
    /// Validate original retained native evidence and acknowledge only a definite
    /// result. No selected provider inherits success from gateway return alone.
    fn after_retained_result(
        &self,
        _resolved: &ToolCall,
        _context: &PreparationContext,
        _evidence: &PreparationEvidence,
        _result: &crate::ToolResult,
        _intent_seq: Option<u64>,
        _result_seq: u64,
    ) -> Result<()> {
        anyhow::bail!("selected retained-result acknowledgement unavailable")
    }
    /// Acquisition may perform bounded network/retained-journal I/O after
    /// connection readiness. It must not send the selected effect.
    fn acquire_before_send(
        &self,
        _resolved: &ToolCall,
        _context: &PreparationContext,
        _evidence: &PreparationEvidence,
    ) -> Result<()> {
        Ok(())
    }
    /// Final local-only check. No network, credential refresh or journal reads.
    fn check_before_send(
        &self,
        resolved: &ToolCall,
        context: &PreparationContext,
        evidence: &PreparationEvidence,
    ) -> Result<()>;
}

pub(crate) struct PreparedDispatch {
    pub intent_seq: Option<u64>,
    pub context: PreparationContext,
    pub prepared: PreparedCall,
}

impl PreparedCall {
    pub fn validate(&self, requested: &ToolCall, context: &PreparationContext) -> Result<()> {
        ensure!(
            self.call.server == requested.server && self.call.tool == requested.tool,
            "preparation cannot change a tool grant"
        );
        ensure!(
            self.call.args.is_object(),
            "prepared arguments must be an object"
        );
        ensure!(
            serde_json::to_vec(&self.call.args)?.len() <= MAX_PREPARATION_BYTES,
            "prepared arguments exceed limit"
        );
        let e = &self.evidence;
        ensure!(
            !e.provider.is_empty() && e.provider.len() <= 256,
            "invalid preparation provider"
        );
        ensure!(
            e.context_sha256 == context.raw()?.sha256,
            "preparation native context mismatch"
        );
        ensure!(
            e.response_json.len() <= MAX_PREPARATION_BYTES,
            "preparation response exceeds limit"
        );
        ensure!(
            e.response_sha256 == raw_sha256(e.response_json.as_bytes()),
            "preparation response hash mismatch"
        );
        let _: serde_json::Value = serde_json::from_str(&e.response_json)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn raw_context_preserves_maximum_u64_without_javascript_number() {
        let c = PreparationContext {
            run_id: "actual-test-run".into(),
            node: "work".into(),
            task_ref: "task".into(),
            model_slot: "coder".into(),
            model_call_ordinal: 1,
            frames: vec![],
            cursors: vec![],
            argument_bindings: vec![],
            call_context: CallContext {
                epoch: "actual-test-epoch".into(),
                attempt: u64::MAX,
                call: u32::MAX,
            },
        };
        let raw = c.raw().unwrap();
        assert!(raw.json.contains("18446744073709551615"));
        let outer = serde_json::to_value(&raw).unwrap();
        assert!(outer["json"].is_string());
        assert_eq!(raw.sha256, raw_sha256(raw.json.as_bytes()));
        assert_eq!(raw.call_context.attempt, "18446744073709551615");
        let requested = ToolCall {
            server: "selected".into(),
            tool: "act".into(),
            args: json!({"purpose":"test"}),
        };
        let response = "{\"args\":{\"target\":\"actual\"}}";
        let mut prepared = PreparedCall {
            call: ToolCall {
                args: json!({"target":"actual"}),
                ..requested.clone()
            },
            evidence: PreparationEvidence {
                provider: "private-test-adapter".into(),
                context_sha256: raw.sha256,
                response_json: response.into(),
                response_sha256: raw_sha256(response.as_bytes()),
            },
        };
        prepared.validate(&requested, &c).unwrap();
        prepared.call.tool = "different".into();
        assert!(prepared.validate(&requested, &c).is_err());
        prepared.call.tool = requested.tool.clone();
        prepared.evidence.context_sha256 = "0".repeat(64);
        assert!(prepared.validate(&requested, &c).is_err());
    }
}
