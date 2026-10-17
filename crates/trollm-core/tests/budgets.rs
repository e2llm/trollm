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

#[test]
fn token_exact_and_over_boundary_obey_run_policy() {
    // MockModel uses whitespace counts: one prompt token + one prose token.
    for policy in ["abort", "respond_partial"] {
        for limit in [1, 2] {
            let base = common::tmp_dir("token-budget");
            std::fs::write(base.join("prompt.md"), "go").unwrap();
            let yaml = plan_yaml(policy).replace(
                "maxTokensTotal: 100000",
                &format!("maxTokensTotal: {limit}"),
            );
            let plan = ExecutionPlan::from_yaml_str(&yaml).unwrap();
            let registry = TaskRegistry::from_yaml_str(
                &REGISTRY.replace("tools: [t.ping]", "tools: []"),
                &base,
            )
            .unwrap();
            let run = RunDir::under(&base);
            let path = run.path().to_path_buf();
            let mut exec = Executor::new(
                plan,
                registry,
                String::new(),
                SlotMap::new().with("coder", MockModel::scripted(["done"])),
                ScriptGateway::default(),
                run,
            )
            .unwrap();
            let status = exec.run_until_blocked().unwrap();
            let events = read_events(&path);
            let exhausted: Vec<_> = events
                .iter()
                .filter(|e| e["type"] == "budget_exhausted")
                .collect();
            if limit == 2 {
                assert_eq!(status, RunStatus::Succeeded);
                assert!(exhausted.is_empty());
                assert_eq!(events.last().unwrap()["outcome"], "succeeded");
            } else {
                assert_eq!(exhausted.len(), 1);
                assert_eq!(exhausted[0]["budget"], "maxTokensTotal");
                assert_eq!(exhausted[0]["value"], 2);
                assert_eq!(exhausted[0]["limit"], 1);
                assert_eq!(
                    matches!(status, RunStatus::Failed { .. }),
                    policy == "abort"
                );
                assert_eq!(
                    events.last().unwrap()["outcome"],
                    if policy == "abort" {
                        "failed"
                    } else {
                        "partial"
                    }
                );
            }
            assert_eq!(exec.state().counters.tokens, 2);
            drop(exec);
            std::fs::remove_dir_all(base).unwrap();
        }
    }
}

#[test]
fn canonical_reasoner_exact_limit_and_next_call_obey_run_policy() {
    for policy in ["abort", "respond_partial"] {
        for limit in [0, 1] {
            let base = common::tmp_dir("reasoner-budget");
            std::fs::write(base.join("prompt.md"), "go").unwrap();
            let yaml = plan_yaml(policy)
                .replace("maxReasonerCalls: 5", &format!("maxReasonerCalls: {limit}"))
                .replace("models: { coder:", "models: { reasoner:");
            let plan = ExecutionPlan::from_yaml_str(&yaml).unwrap();
            let registry = TaskRegistry::from_yaml_str(
                &REGISTRY
                    .replace("model: coder", "model: reasoner")
                    .replace("tools: [t.ping]", "tools: []"),
                &base,
            )
            .unwrap();
            let run = RunDir::under(&base);
            let path = run.path().to_path_buf();
            let mut exec = Executor::new(
                plan,
                registry,
                String::new(),
                SlotMap::new().with("reasoner", MockModel::scripted(["done"])),
                ScriptGateway::default(),
                run,
            )
            .unwrap();
            let status = exec.run_until_blocked().unwrap();
            let events = read_events(&path);
            let exhausted: Vec<_> = events
                .iter()
                .filter(|e| e["type"] == "budget_exhausted")
                .collect();
            assert_eq!(exec.state().counters.reasoner_calls, limit);
            if limit == 1 {
                assert_eq!(status, RunStatus::Succeeded);
                assert!(exhausted.is_empty());
            } else {
                assert_eq!(exhausted.len(), 1);
                assert_eq!(exhausted[0]["budget"], "maxReasonerCalls");
                assert_eq!(exhausted[0]["value"], 0);
                assert_eq!(exhausted[0]["limit"], 0);
                assert_eq!(
                    matches!(status, RunStatus::Failed { .. }),
                    policy == "abort"
                );
                assert_eq!(
                    events.last().unwrap()["outcome"],
                    if policy == "abort" {
                        "failed"
                    } else {
                        "partial"
                    }
                );
            }
            drop(exec);
            std::fs::remove_dir_all(base).unwrap();
        }
    }
}

#[test]
fn gateway_artifact_exact_zero_and_over_obey_run_policy() {
    for policy in ["abort", "respond_partial"] {
        for produces_bytes in [false, true] {
            let base = common::tmp_dir("gateway-artifact-budget");
            std::fs::write(base.join("prompt.md"), "go").unwrap();
            let plan = ExecutionPlan::from_yaml_str(
                &plan_yaml(policy).replace("artifactMaxMb: 5", "artifactMaxMb: 0"),
            )
            .unwrap();
            let registry = TaskRegistry::from_yaml_str(
                r#"
schemaVersion: ledger.task-registry.v1
tasks:
  loop-forever:
    model: coder
    prompt: prompt.md
    tools: [t.ping]
    completion: tool-result
    outputContract: { outputs: {} }
"#,
                &base,
            )
            .unwrap();
            let run = RunDir::under(&base);
            let path = run.path().to_path_buf();
            let mut gateway = ScriptGateway::new(run.artifacts_dir());
            gateway.artifact_tools.insert("t.ping".into());
            if produces_bytes {
                gateway.items.push(serde_json::json!(null));
            }
            let mut exec = Executor::new(
                plan,
                registry,
                String::new(),
                SlotMap::new().with(
                    "coder",
                    MockModel::scripted([
                        r#"<tool_call>{"server":"t","tool":"ping","args":{}}</tool_call>"#,
                    ]),
                ),
                gateway,
                run,
            )
            .unwrap();
            let status = exec.run_until_blocked().unwrap();
            let events = read_events(&path);
            let bytes = if produces_bytes { 5 } else { 0 };
            assert_eq!(exec.state().counters.artifact_bytes, bytes);
            assert_eq!(exec.state().stack[0].counters.artifact_bytes, bytes);
            let exhausted: Vec<_> = events
                .iter()
                .filter(|e| e["type"] == "budget_exhausted")
                .collect();
            if produces_bytes {
                assert_eq!(exhausted.len(), 1);
                assert_eq!(exhausted[0]["budget"], "artifactMaxMb");
                assert_eq!(exhausted[0]["value"], bytes);
                assert_eq!(exhausted[0]["limit"], 0);
                assert_eq!(
                    matches!(status, RunStatus::Failed { .. }),
                    policy == "abort"
                );
                assert_eq!(
                    events.last().unwrap()["outcome"],
                    if policy == "abort" {
                        "failed"
                    } else {
                        "partial"
                    }
                );
            } else {
                assert_eq!(status, RunStatus::Succeeded);
                assert!(exhausted.is_empty());
            }
            drop(exec);
            std::fs::remove_dir_all(base).unwrap();
        }
    }
}
