//! onError retry path (§4.5): retry with backoff, then route to `then`;
//! default policy when onError is absent.

mod common;

use common::{event_types, read_events, transitions, ScriptGateway};
use trollm_core::{ExecutionPlan, Executor, RunDir, RunStatus, SlotMap, TaskRegistry};
use trollm_models::MockModel;

fn plan_yaml(on_error: &str) -> String {
    format!(
        r#"
schemaVersion: ledger.execution-plan.v1
id: retry-test
version: 1
sourceBaseline: {{ sha256: "{}" }}
input: {{ promptTemplate: "go" }}
limits: {{ maxSteps: 20, maxReasonerCalls: 5, maxTokensTotal: 100000, timeoutS: 600, artifactMaxMb: 5 }}
onLimit: abort
models: {{ coder: {{ ctx: 8192 }} }}
mcp: {{ servers: [], policies: {{}} }}
entryStep: work
nodes:
  work:
    kind: task
    taskRef: flaky-task
    onVerified: done
    {on_error}
  done: {{ kind: end, outcome: succeeded }}
  fail: {{ kind: end, outcome: failed }}
"#,
        "0".repeat(64)
    )
}

const REGISTRY: &str = r#"
schemaVersion: ledger.task-registry.v1
tasks:
  flaky-task:
    model: coder
    prompt: prompt.md
    tools: [t.flaky]
    loop: { maxSteps: 4 }
    outputContract:
      outputs: { outcome: { from: model, type: prose } }
"#;

const FENCE: &str = r#"<tool_call>{"server":"t","tool":"flaky","args":{"x":1}}</tool_call>"#;

#[test]
fn retry_backoff_bounds_apply_to_root_and_nested_plans() {
    use trollm_core::plan::MAX_ON_ERROR_BACKOFF_S;
    use trollm_core::validate::validate_plan;

    for delay in [
        0,
        MAX_ON_ERROR_BACKOFF_S,
        MAX_ON_ERROR_BACKOFF_S + 1,
        u64::MAX,
    ] {
        let policy = format!("onError: {{ retry: 1, backoffS: {delay}, then: fail }}");
        let child = ExecutionPlan::from_yaml_str(&plan_yaml(&policy)).unwrap();
        let expected_valid = delay <= MAX_ON_ERROR_BACKOFF_S;
        assert_eq!(
            validate_plan(&child).is_ok(),
            expected_valid,
            "root: {delay}"
        );

        // Reach the invalid delay only through two levels of loaded subflows.
        let mut parent = ExecutionPlan::from_yaml_str(&plan_yaml("")).unwrap();
        parent.id = "parent".into();
        parent.subflows.insert(child.id.clone(), child);
        let mut root = ExecutionPlan::from_yaml_str(&plan_yaml("")).unwrap();
        root.id = "root".into();
        root.subflows.insert(parent.id.clone(), parent);
        let result = validate_plan(&root);
        assert_eq!(result.is_ok(), expected_valid, "nested: {delay}");
        if let Err(error) = result {
            let detail = error.to_string();
            assert!(detail.contains("retry-test"), "{detail}");
            assert!(detail.contains("onError.backoffS"), "{detail}");
        }
    }
    assert!(ExecutionPlan::from_yaml_str(&plan_yaml(
        "onError: { retry: 1, backoffS: -1, then: fail }"
    ))
    .is_err());
}

fn setup(
    on_error: &str,
    script: MockModel,
    mut gateway: ScriptGateway,
    flaky_fails: &str,
) -> (Executor<MockModel, ScriptGateway>, std::path::PathBuf) {
    let base = common::tmp_dir("retry");
    std::fs::write(base.join("prompt.md"), "do the thing").unwrap();
    let plan = ExecutionPlan::from_yaml_str(&plan_yaml(on_error)).unwrap();
    let registry = TaskRegistry::from_yaml_str(REGISTRY, &base).unwrap();
    match flaky_fails {
        "once" => {
            gateway.fail_once.insert("t.flaky".to_string());
        }
        "always" => {
            gateway.fail_always.insert("t.flaky".to_string());
        }
        _ => {}
    }
    let models = SlotMap::new().with("coder", script);
    let run_dir = RunDir::under(&base);
    let path = run_dir.path().to_path_buf();
    let exec = Executor::new(plan, registry, String::new(), models, gateway, run_dir).unwrap();
    (exec, path)
}

#[test]
fn on_error_retries_then_succeeds() {
    // gen1: fence -> gateway fails (mcp-error) -> retry -> gen2: fence -> ok ->
    // gen3: prose -> verified.
    let script = MockModel::scripted([FENCE, FENCE, "done after retry"]);
    let (mut exec, path) = setup(
        "onError: { retry: 1, backoffS: 0, then: fail }",
        script,
        ScriptGateway::default(),
        "once",
    );
    let status = exec.run_until_blocked().unwrap();
    assert_eq!(status, RunStatus::Succeeded);

    let events = read_events(&path);
    let errors: Vec<_> = events.iter().filter(|e| e["type"] == "error").collect();
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0]["class"], "mcp-error");
    let flaky_calls = events
        .iter()
        .filter(|e| e["type"] == "tool_call_result" && e["tool"] == "flaky")
        .count();
    assert_eq!(flaky_calls, 1, "only the retry produced a result");
    let trs = transitions(&events);
    assert!(trs
        .iter()
        .any(|t| t.1 == "work" && t.2 == "done" && t.3 == "onVerified"));
}

#[test]
fn on_error_retries_exhausted_routes_to_then() {
    let script = MockModel::scripted([FENCE, FENCE, FENCE, FENCE]);
    let (mut exec, path) = setup(
        "onError: { retry: 1, backoffS: 0, then: fail }",
        script,
        ScriptGateway::default(),
        "always",
    );
    let status = exec.run_until_blocked().unwrap();
    assert_eq!(
        status,
        RunStatus::Failed {
            reason: "failed".into()
        }
    );

    let events = read_events(&path);
    let errors: Vec<_> = events.iter().filter(|e| e["type"] == "error").collect();
    assert_eq!(errors.len(), 2, "initial attempt + one retry");
    let trs = transitions(&events);
    assert!(trs
        .iter()
        .any(|t| t.1 == "work" && t.2 == "fail" && t.3 == "onError"));
    assert_eq!(events.last().unwrap()["outcome"], "failed");
}

#[test]
fn on_error_default_fails_with_error_class_reason() {
    // No onError declared: default is retry 0, then {status: failed, reason:
    // <error-class>} (§4.5).
    let script = MockModel::scripted([FENCE]);
    let (mut exec, path) = setup("", script, ScriptGateway::default(), "always");
    let status = exec.run_until_blocked().unwrap();
    match status {
        RunStatus::Failed { reason } => assert_eq!(reason, "mcp-error"),
        other => panic!("expected Failed, got {other:?}"),
    }
    let types = event_types(&read_events(&path));
    assert!(types.iter().any(|t| t == "error"));
}

#[test]
fn malformed_fence_is_tool_call_parse() {
    let script = MockModel::scripted(["<tool_call>not json</tool_call>"]);
    let (mut exec, path) = setup("", script, ScriptGateway::default(), "never");
    let status = exec.run_until_blocked().unwrap();
    match status {
        RunStatus::Failed { reason } => assert_eq!(reason, "tool-call-parse"),
        other => panic!("expected Failed, got {other:?}"),
    }
    let events = read_events(&path);
    let errors: Vec<_> = events.iter().filter(|e| e["type"] == "error").collect();
    assert_eq!(errors[0]["class"], "tool-call-parse");
}
