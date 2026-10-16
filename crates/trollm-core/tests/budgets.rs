//! Budget exhaustion → run-level onLimit (§3, §4.4). Run-level budgets live in
//! the frame's counter scope and never reset; per-node loop budgets reset on
//! node entry.

mod common;

use common::{event_types, read_events, ScriptGateway};
use trollm_core::{ExecutionPlan, Executor, RunDir, RunStatus, SlotMap, TaskRegistry};
use trollm_models::MockModel;

fn plan_yaml(on_limit: &str) -> String {
    format!(
        r#"
schemaVersion: ledger.execution-plan.v1
id: budget-test
version: 1
sourceBaseline: {{ sha256: "{}" }}
input: {{ promptTemplate: "go" }}
limits: {{ maxSteps: 2, maxReasonerCalls: 5, maxTokensTotal: 100000, timeoutS: 600, artifactMaxMb: 5 }}
onLimit: {on_limit}
models: {{ coder: {{ ctx: 8192 }} }}
mcp: {{ servers: [], policies: {{}} }}
entryStep: work
nodes:
  work: {{ kind: task, taskRef: loop-forever, onVerified: done }}
  done: {{ kind: end, outcome: succeeded }}
"#,
        "0".repeat(64)
    )
}

const REGISTRY: &str = r#"
schemaVersion: ledger.task-registry.v1
tasks:
  loop-forever:
    model: coder
    prompt: prompt.md
    tools: [t.ping]
    loop: { maxSteps: 5 }
    outputContract:
      outputs: { outcome: { from: model, type: prose } }
"#;

fn setup(on_limit: &str) -> (Executor<MockModel, ScriptGateway>, std::path::PathBuf) {
    let base = common::tmp_dir("budget");
    std::fs::write(base.join("prompt.md"), "ping").unwrap();
    let plan = ExecutionPlan::from_yaml_str(&plan_yaml(on_limit)).unwrap();
    let registry = TaskRegistry::from_yaml_str(REGISTRY, &base).unwrap();
    // The model would keep emitting fences forever; the budget must stop it.
    let coder = MockModel::scripted([
        r#"<tool_call>{"server":"t","tool":"ping","args":{}}</tool_call>"#,
        r#"<tool_call>{"server":"t","tool":"ping","args":{}}</tool_call>"#,
        r#"<tool_call>{"server":"t","tool":"ping","args":{}}</tool_call>"#,
    ]);
    let models = SlotMap::new().with("coder", coder);
    let run_dir = RunDir::under(&base);
    let path = run_dir.path().to_path_buf();
    let exec = Executor::new(
        plan,
        registry,
        String::new(),
        models,
        ScriptGateway::default(),
        run_dir,
    )
    .unwrap();
    (exec, path)
}

#[test]
fn budget_exhaustion_aborts_when_on_limit_abort() {
    let (mut exec, path) = setup("abort");
    let status = exec.run_until_blocked().unwrap();
    match status {
        RunStatus::Failed { reason } => assert!(reason.contains("budget-exhausted"), "{reason}"),
        other => panic!("expected Failed, got {other:?}"),
    }
    let events = read_events(&path);
    let exhausted: Vec<_> = events
        .iter()
        .filter(|e| e["type"] == "budget_exhausted")
        .collect();
    assert_eq!(exhausted.len(), 1);
    assert_eq!(exhausted[0]["budget"], "maxSteps");
    assert_eq!(events.last().unwrap()["type"], "run_ended");
    assert_eq!(events.last().unwrap()["outcome"], "failed");
    // exactly maxSteps generations happened, no more
    let model_calls = event_types(&events)
        .iter()
        .filter(|t| *t == "model_call")
        .count();
    assert_eq!(model_calls, 2);
}

#[test]
fn budget_exhaustion_responds_partial_when_configured() {
    let (mut exec, path) = setup("respond_partial");
    let status = exec.run_until_blocked().unwrap();
    assert_eq!(status, RunStatus::Succeeded, "respond_partial ends the run");
    let events = read_events(&path);
    assert_eq!(events.last().unwrap()["outcome"], "partial");
}
