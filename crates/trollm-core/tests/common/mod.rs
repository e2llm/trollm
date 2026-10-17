//! Shared test harness: a scripted in-test ToolGateway (§11: "conformance
//! tests executing flows/examples/* end-to-end with MockModel + scripted mock
//! gateway"). No network, no real models.
//!
//! Each test binary compiles this module separately; not every binary uses
//! every helper.
#![allow(dead_code)]

use anyhow::{anyhow, Result};
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use trollm_core::artifacts::{self, ArtifactMeta};
use trollm_mcp::{Provenance, ToolCall, ToolDescriptor, ToolGateway, ToolResult};

/// A gateway scripted per-test. Behavior:
/// - `list_issues`-style read tools listed in `artifact_tools` write their
///   configured items to `<server>.<tool>.latest.jsonl` (+ meta sidecar,
///   `complete: true`) in the run's artifact dir and report it in
///   `ToolResult.artifacts` — mirroring the §6 paginate/backgroundFetch shape.
/// - `record_tools` (e.g. get_issue) answer from `items` by matching `id`.
/// - everything else is treated as a mutation: recorded, `{ok: true}`.
/// - `fail_once` tools error on their first invocation (mcp-error path).
/// - `crash_on` tools panic mid-call, simulating a process crash between
///   `tool_call_intent` and `tool_call_result` (§6).
#[derive(Default)]
pub struct ScriptGateway {
    pub artifacts_dir: PathBuf,
    pub items: Vec<Value>,
    /// `<server>.<tool>` fqns that produce the item-list artifact.
    pub artifact_tools: BTreeSet<String>,
    /// fqns that answer a single record looked up by `args.id`.
    pub record_tools: BTreeSet<String>,
    pub fail_once: BTreeSet<String>,
    pub fail_always: BTreeSet<String>,
    pub crash_on: Option<String>,
    pub calls: Vec<ToolCall>,
}

impl ScriptGateway {
    pub fn new(artifacts_dir: PathBuf) -> Self {
        Self {
            artifacts_dir,
            ..Default::default()
        }
    }
}

impl ToolGateway for ScriptGateway {
    fn tool_class(&self, _: &str, _: &str) -> trollm_mcp::policy::ToolClass {
        // Classification comes from the fixture plan; generic fake effects
        // alone do not assign an effective gateway mutation policy.
        trollm_mcp::policy::ToolClass::Read
    }
    fn list_tools(&self) -> Result<Vec<ToolDescriptor>> {
        Ok(vec![])
    }

    fn validate_call(&self, _: &ToolCall) -> Result<()> {
        // This fixture defines every tool as an artifact read, record read,
        // or generic mutation; scripted transport failures occur on dispatch.
        Ok(())
    }

    fn call(&mut self, call: &ToolCall) -> Result<ToolResult> {
        let fqn = format!("{}.{}", call.server, call.tool);
        self.calls.push(call.clone());

        if self.crash_on.as_deref() == Some(fqn.as_str()) {
            // Simulated crash: the core has already persisted tool_call_intent
            // (it journals BEFORE the call); we die before any result exists.
            panic!("simulated crash before tool_call_result for {fqn}");
        }
        if self.fail_always.contains(&fqn) || self.fail_once.remove(&fqn) {
            return Err(anyhow!("scripted failure of {fqn}"));
        }

        if self.artifact_tools.contains(&fqn) {
            let name = format!("{fqn}.latest.jsonl");
            let mut buf = String::new();
            for item in &self.items {
                buf.push_str(&item.to_string());
                buf.push('\n');
            }
            let written = artifacts::write_atomic(&self.artifacts_dir, &name, buf.as_bytes())?;
            artifacts::write_meta(
                &written.path,
                &ArtifactMeta {
                    fetched_at: 0,
                    complete: true,
                },
            )?;
            return Ok(ToolResult {
                content_for_model: json!({
                    "page": 1,
                    "note": "page 1 delivered; full listing fetched to file",
                    "count": self.items.len(),
                }),
                artifacts: vec![written.path],
                provenance: Provenance::Live,
            });
        }

        if self.record_tools.contains(&fqn) {
            let id = call.args.get("id").cloned().unwrap_or(Value::Null);
            let rec = self
                .items
                .iter()
                .find(|it| it.get("id") == Some(&id))
                .cloned()
                .ok_or_else(|| anyhow!("no record for id {id}"))?;
            return Ok(ToolResult {
                content_for_model: rec,
                artifacts: vec![],
                provenance: Provenance::ServedFromFile,
            });
        }

        // Mutation passthrough (§6).
        Ok(ToolResult {
            content_for_model: json!({ "ok": true }),
            artifacts: vec![],
            provenance: Provenance::Live,
        })
    }
}

/// Read a run's events.jsonl as parsed values.
pub fn read_events(run_dir: &Path) -> Vec<Value> {
    let text = std::fs::read_to_string(run_dir.join("events.jsonl")).unwrap();
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

pub fn event_types(events: &[Value]) -> Vec<String> {
    events
        .iter()
        .map(|e| e["type"].as_str().unwrap().to_string())
        .collect()
}

/// (plan, from, to, key) for every transition event.
pub fn transitions(events: &[Value]) -> Vec<(String, String, String, String)> {
    events
        .iter()
        .filter(|e| e["type"] == "transition")
        .map(|e| {
            (
                e["plan"].as_str().unwrap_or("").to_string(),
                e["from"].as_str().unwrap_or("").to_string(),
                e["to"].as_str().unwrap_or("").to_string(),
                e["key"].as_str().unwrap_or("").to_string(),
            )
        })
        .collect()
}

pub fn tmp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("trollm-core-test-{tag}-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}
