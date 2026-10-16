mod common;

use anyhow::{ensure, Context, Result};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use trollm_core::{ExecutionPlan, Executor, RunDir, RunStatus, SlotMap, TaskRegistry};
use trollm_mcp::{Provenance, ToolCall, ToolDescriptor, ToolGateway, ToolResult};
use trollm_models::MockModel;

const PLAN: &str = r#"
schemaVersion: ledger.execution-plan.v1
id: preflight
version: 1
limits: { maxSteps: 10, maxReasonerCalls: 0, maxTokensTotal: 10000, timeoutS: 60, artifactMaxMb: 1 }
onLimit: abort
models: { coder: { ctx: 2048 } }
entryStep: act
nodes:
  act: { kind: task, taskRef: mutate, onVerified: done }
  done: { kind: end, outcome: succeeded }
"#;

const REGISTRY: &str = r#"
schemaVersion: ledger.task-registry.v1
tasks:
  mutate:
    model: coder
    prompt: mutate.md
    tools: [t.write]
    loop: { maxSteps: 3 }
    mutations: { idempotency: intent-log, onUncertain: fail-run }
    outputContract:
      outputs: { outcome: { from: model, type: prose } }
"#;

struct CheckedMutationGateway {
    run_path: PathBuf,
    validations: Arc<AtomicUsize>,
    calls: Arc<AtomicUsize>,
}

impl ToolGateway for CheckedMutationGateway {
    fn list_tools(&self) -> Result<Vec<ToolDescriptor>> {
        Ok(vec![])
    }

    fn validate_call(&self, call: &ToolCall) -> Result<()> {
        self.validations.fetch_add(1, Ordering::SeqCst);
        assert!(common::read_events(&self.run_path)
            .iter()
            .all(|event| event["type"] != "tool_call_intent"));
        ensure!(call.server == "t" && call.tool == "write", "unknown tool");
        if call.args["discoveryFailure"] == true {
            return Err(anyhow::anyhow!("MCP tools/list failed: HTTP 401"))
                .context("MCP tool discovery failed for 't'");
        }
        ensure!(call.args["id"].is_string(), "id must be a string");
        Ok(())
    }

    fn call(&mut self, call: &ToolCall) -> Result<ToolResult> {
        // Read from the actual event file during dispatch, proving that the
        // intent has been persisted before the mutation reaches the gateway.
        let events = common::read_events(&self.run_path);
        let intents: Vec<_> = events
            .iter()
            .filter(|event| event["type"] == "tool_call_intent")
            .collect();
        assert_eq!(intents.len(), 1);
        assert_eq!(intents[0]["args"], call.args);
        assert!(events
            .iter()
            .all(|event| event["type"] != "tool_call_result"));
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(ToolResult {
            content_for_model: json!({"ok": true}),
            artifacts: vec![],
            provenance: Provenance::Live,
        })
    }
}

fn run_mutation(tool: &str, args: Value) -> (RunStatus, usize, usize, Vec<Value>) {
    let base = common::tmp_dir("tool-preflight");
    std::fs::write(base.join("mutate.md"), "perform mutation").unwrap();
    let run_dir = RunDir::under(&base);
    let validations = Arc::new(AtomicUsize::new(0));
    let calls = Arc::new(AtomicUsize::new(0));
    let fence = format!(
        "<tool_call>{}</tool_call>",
        json!({"server": "t", "tool": tool, "args": args})
    );
    let gateway = CheckedMutationGateway {
        run_path: run_dir.path().to_path_buf(),
        validations: validations.clone(),
        calls: calls.clone(),
    };
    let mut exec = Executor::new(
        ExecutionPlan::from_yaml_str(PLAN).unwrap(),
        TaskRegistry::from_yaml_str(REGISTRY, &base).unwrap(),
        String::new(),
        SlotMap::new().with("coder", MockModel::scripted([fence, "done".into()])),
        gateway,
        run_dir.clone(),
    )
    .unwrap();
    let status = exec.run_until_blocked().unwrap();
    let events = common::read_events(run_dir.path());
    drop(exec);
    std::fs::remove_dir_all(base).unwrap();
    (
        status,
        validations.load(Ordering::SeqCst),
        calls.load(Ordering::SeqCst),
        events,
    )
}

#[test]
fn invalid_granted_mutation_has_no_intent_or_dispatch() {
    let (status, validations, calls, events) = run_mutation("write", json!({"id": 7}));
    assert!(matches!(status, RunStatus::Failed { reason } if reason.contains("tool-call-parse")));
    assert_eq!(validations, 1);
    assert_eq!(calls, 0);
    assert!(events
        .iter()
        .all(|event| event["type"] != "tool_call_intent"));
    assert!(events
        .iter()
        .any(|event| event["type"] == "error" && event["class"] == "tool-call-parse"));
}

#[test]
fn denied_tool_never_reaches_preflight() {
    let (status, validations, calls, events) = run_mutation("denied", json!({"id": "7"}));
    assert!(matches!(status, RunStatus::Failed { reason } if reason.contains("tool-call-parse")));
    assert_eq!(validations, 0);
    assert_eq!(calls, 0);
    assert!(events
        .iter()
        .all(|event| event["type"] != "tool_call_intent"));
}

#[test]
fn discovery_cause_reaches_error_event_without_intent_or_dispatch() {
    let (status, validations, calls, events) =
        run_mutation("write", json!({"id": "7", "discoveryFailure": true}));
    assert!(matches!(status, RunStatus::Failed { .. }));
    assert_eq!(validations, 1);
    assert_eq!(calls, 0);
    assert!(events
        .iter()
        .all(|event| event["type"] != "tool_call_intent"));
    let error = events
        .iter()
        .find(|event| event["type"] == "error")
        .unwrap();
    assert_eq!(error["class"], "tool-call-parse");
    assert_eq!(
        error["detail"],
        "t.write: MCP tool discovery failed for 't': MCP tools/list failed: HTTP 401"
    );
}

#[test]
fn valid_mutation_has_persisted_intent_before_dispatch() {
    let (status, validations, calls, events) = run_mutation("write", json!({"id": "7"}));
    assert!(matches!(status, RunStatus::Succeeded));
    assert_eq!(validations, 1);
    assert_eq!(calls, 1);
    assert_eq!(
        events
            .iter()
            .filter(|event| event["type"] == "tool_call_result")
            .count(),
        1
    );
}
