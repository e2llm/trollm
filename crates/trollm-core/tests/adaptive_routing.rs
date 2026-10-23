//! Generic graph routing with scripted models and a host-only gateway.
mod common;
use anyhow::Result;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use trollm_core::{ExecutionPlan, Executor, RunDir, RunStatus, SlotMap, TaskRegistry};
use trollm_models::{GenerateRequest, GenerateResponse, MockModel, ModelSlot};

const PLAN: &str = r#"
schemaVersion: ledger.execution-plan.v1
id: routing
version: 1
limits: { maxSteps: 4, maxReasonerCalls: 0, maxTokensTotal: 10000, timeoutS: 60, artifactMaxMb: 1 }
onLimit: abort
models: { coder: { ctx: 2048 } }
entryStep: choose
nodes:
  choose:
    kind: task
    taskRef: choose
    promptSupplement: 'Choose at {{ runtime.node }}'
    routes: { finish: done, again: choose, pause: { status: paused, reason: chosen, resumeAt: done } }
  done: { kind: end, outcome: succeeded }
"#;
const REGISTRY: &str = r#"
schemaVersion: ledger.task-registry.v1
tasks:
  choose:
    model: coder
    prompt: prompt.md
    tools: [flow.choose, t.read]
    completion: flow-route
    loop: { maxSteps: 2 }
    maxOutputTokens: 80
    generationTimeoutS: 5
"#;
const CHOOSE: &str =
    r#"<tool_call>{"server":"flow","tool":"choose","args":{"route":"finish"}}</tool_call>"#;
const READ: &str = r#"<tool_call>{"server":"t","tool":"read","args":{}}</tool_call>"#;
struct Recorder {
    mock: MockModel,
    requests: Arc<Mutex<Vec<GenerateRequest>>>,
}
impl ModelSlot for Recorder {
    fn generate(&mut self, req: GenerateRequest) -> Result<GenerateResponse> {
        self.requests.lock().unwrap().push(req.clone());
        self.mock.generate(req)
    }
}
fn run(
    plan: &str,
    registry: &str,
    responses: &[&str],
) -> (RunStatus, Value, Vec<Value>, Vec<GenerateRequest>) {
    run_inner(plan, registry, responses, false)
}
fn run_inner(
    plan: &str,
    registry: &str,
    responses: &[&str],
    fail: bool,
) -> (RunStatus, Value, Vec<Value>, Vec<GenerateRequest>) {
    let base = common::tmp_dir("adaptive-routing");
    std::fs::write(base.join("prompt.md"), "Registry {{ user_input }}").unwrap();
    let dir = RunDir::under(&base);
    let requests = Arc::new(Mutex::new(vec![]));
    let mut gateway = common::ScriptGateway::new(dir.artifacts_dir());
    if fail {
        gateway.fail_always.insert("t.read".into());
    }
    let mut exec = Executor::new(
        ExecutionPlan::from_yaml_str(plan).unwrap(),
        TaskRegistry::from_yaml_str(registry, &base).unwrap(),
        "input".into(),
        SlotMap::new().with(
            "coder",
            Recorder {
                mock: MockModel::scripted(responses.iter().copied()),
                requests: requests.clone(),
            },
        ),
        gateway,
        dir.clone(),
    )
    .unwrap();
    let status = exec.run_until_blocked().unwrap();
    let state = serde_json::to_value(exec.state()).unwrap();
    let events = common::read_events(dir.path());
    let requests = requests.lock().unwrap().clone();
    std::fs::remove_dir_all(base).unwrap();
    (status, state, events, requests)
}
#[test]
fn explicit_route_context_and_generation_limits() {
    let (status, state, events, reqs) = run(PLAN, REGISTRY, &[CHOOSE]);
    assert_eq!(status, RunStatus::Succeeded);
    assert_eq!(state["counters"]["steps"], 1);
    assert!(events
        .iter()
        .any(|e| e["type"] == "transition" && e["key"] == "routes.finish"));
    assert!(!events.iter().any(|e| e["type"] == "tool_call_result"));
    assert!(reqs[0].prompt.contains("Choose at choose"));
    assert!(reqs[0].prompt.contains("\"finish\":\"done\""));
    assert_eq!(reqs[0].max_tokens, 80);
    assert_eq!(
        reqs[0].max_duration,
        Some(std::time::Duration::from_secs(5))
    );
}
#[test]
fn malformed_or_mixed_routes_fail_before_any_action() {
    for text in [
        format!("{READ}{CHOOSE}"),
        format!("{CHOOSE}{READ}"),
        CHOOSE.replace("finish", "invented"),
        CHOOSE.replace("choose", "unknown"),
        CHOOSE.replace("\"finish\"", "\"finish\",\"extra\":1"),
        "Finished".into(),
    ] {
        let (status, _, events, _) = run(PLAN, REGISTRY, &[&text]);
        assert!(matches!(status, RunStatus::Failed { .. }));
        assert!(!events
            .iter()
            .any(|e| e["type"] == "tool_call_result" || e["type"] == "tool_call_intent"));
    }
}
#[test]
fn route_cannot_bypass_artifact_verification() {
    let registry =
        format!("{REGISTRY}\n    outputContract: {{ requireArtifactComplete: [missing] }}\n");
    let (status, _, events, _) = run(PLAN, &registry, &[CHOOSE]);
    assert!(matches!(status, RunStatus::Failed { .. }));
    assert!(!events.iter().any(|e| e["key"] == "routes.finish"));
}
#[test]
fn cycles_and_action_loop_remain_bounded() {
    let again = CHOOSE.replace("finish", "again");
    let (status, state, _, _) = run(PLAN, REGISTRY, &[&again, &again, &again, &again]);
    assert!(matches!(status, RunStatus::Failed { reason } if reason.contains("budget-exhausted")));
    assert_eq!(state["counters"]["steps"], 4);
    let (status, _, _, _) = run(PLAN, REGISTRY, &[READ, READ]);
    assert!(
        matches!(status, RunStatus::Failed { reason } if reason.contains("verification-failed"))
    );
}
#[test]
fn error_context_survives_success_and_is_bounded() {
    let plan = PLAN.replace("routes: { finish:", "onError: { then: recover }\n    routes: { finish:")
        .replace("  done:", "  recover: { kind: task, taskRef: choose, promptSupplement: 'Recover {{ runtime.lastError.node }}', routes: { finish: done } }\n  done:");
    let (status, state, _, reqs) = run(&plan, REGISTRY, &["invalid", CHOOSE]);
    assert_eq!(status, RunStatus::Succeeded);
    assert_eq!(state["stack"][0]["lastError"]["node"], "choose");
    assert_eq!(state["stack"][0]["lastError"]["mutationOutcome"], "none");
    assert!(reqs[1].prompt.contains("Recover choose"));
}
#[test]
fn legacy_prompt_is_byte_identical() {
    let plan = PLAN.replace("    promptSupplement: 'Choose at {{ runtime.node }}'\n", "").replace("    routes: { finish: done, again: choose, pause: { status: paused, reason: chosen, resumeAt: done } }", "    onVerified: done");
    let registry = REGISTRY
        .replace("    completion: flow-route\n", "")
        .replace("tools: [flow.choose, t.read]", "tools: []");
    let (status, _, _, reqs) = run(&plan, &registry, &["done"]);
    assert_eq!(status, RunStatus::Succeeded);
    assert_eq!(reqs[0].prompt, "Registry input");
}
#[test]
fn static_contract_rejects_non_coder_and_invalid_limits() {
    for registry in [
        REGISTRY.replace("model: coder", "model: other"),
        REGISTRY.replace("80", "0"),
        REGISTRY.replace("TimeoutS: 5", "TimeoutS: 601"),
        format!("{REGISTRY}\n    toolArguments: {{ t.read: {{fixed: {{x: 1}}}} }}"),
        format!(
            "{REGISTRY}\n    outputContract: {{ outputs: {{x: {{from: model, type: prose}}}} }}"
        ),
    ] {
        assert!(TaskRegistry::from_yaml_str(&registry, ".").is_err());
    }
    let old: trollm_core::state::Frame =
        serde_json::from_value(json!({"plan":"p", "node":"n"})).unwrap();
    assert!(old.last_error.is_none());
}

#[test]
fn mutation_failure_context_distinguishes_returned_and_unknown() {
    let plan = PLAN
        .replace(
            "entryStep: choose",
            "mcp: { policies: { t.read: { class: mutation } } }\nentryStep: choose",
        )
        .replace("    routes:", "    onError: { then: recover }\n    routes:")
        .replace(
            "  done:",
            "  recover: { kind: task, taskRef: choose, routes: { finish: done } }\n  done:",
        );
    let registry = format!(
        "{REGISTRY}\n    mutations: {{ idempotency: intent-log, onUncertain: fail-run }}\n"
    );
    for (fail, responses, expected) in [
        (true, vec![READ, CHOOSE], "unknown"),
        (false, vec![READ, "invalid", CHOOSE], "returned"),
    ] {
        let (status, state, _, reqs) = run_inner(&plan, &registry, &responses, fail);
        assert_eq!(status, RunStatus::Succeeded);
        assert_eq!(state["stack"][0]["lastError"]["mutationOutcome"], expected);
        assert!(reqs
            .last()
            .unwrap()
            .prompt
            .contains(&format!("\"mutationOutcome\":\"{expected}\"")));
    }
}
#[test]
fn committed_route_transition_is_not_repeated_on_resume() {
    let base = common::tmp_dir("route-resume");
    std::fs::write(base.join("prompt.md"), "Choose").unwrap();
    let run_dir = RunDir::under(&base);
    let plan = PLAN.replace("finish: done", "finish: next").replace(
        "  done:",
        "  next: { kind: task, taskRef: next, onVerified: done }\n  done:",
    );
    let registry = format!("{REGISTRY}\n  next: {{ model: coder, prompt: prompt.md }}\n");
    struct CrashModel {
        mock: MockModel,
        crash_after: usize,
    }
    impl ModelSlot for CrashModel {
        fn generate(&mut self, req: GenerateRequest) -> Result<GenerateResponse> {
            assert!(
                self.crash_after > 0,
                "simulated crash after committed route"
            );
            self.crash_after -= 1;
            self.mock.generate(req)
        }
    }
    let create = |responses: Vec<&str>, crash_after| {
        Executor::new(
            ExecutionPlan::from_yaml_str(&plan).unwrap(),
            TaskRegistry::from_yaml_str(&registry, &base).unwrap(),
            String::new(),
            SlotMap::new().with(
                "coder",
                CrashModel {
                    mock: MockModel::scripted(responses),
                    crash_after,
                },
            ),
            common::ScriptGateway::new(run_dir.artifacts_dir()),
            run_dir.clone(),
        )
        .unwrap()
    };
    let mut exec = create(vec![CHOOSE], 1);
    // One advance commits the route boundary, before the next task executes.
    assert!(
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| exec.run_until_blocked()))
            .is_err()
    );
    assert_eq!(exec.state().stack[0].node, "next");
    drop(exec);
    let mut restored = create(vec!["done"], 1);
    assert_eq!(restored.run_until_blocked().unwrap(), RunStatus::Succeeded);
    assert_eq!(
        common::read_events(run_dir.path())
            .iter()
            .filter(|e| e["key"] == "routes.finish")
            .count(),
        1
    );
    std::fs::remove_dir_all(base).unwrap();
}

#[test]
fn persisted_error_detail_is_utf8_bounded() {
    let variable = "🦀".repeat(1200);
    let plan = PLAN.replace(
        "'Choose at {{ runtime.node }}'",
        &format!("'{{{{ {variable} }}}}'"),
    );
    let (_, state, _, requests) = run(&plan, REGISTRY, &[]);
    assert!(requests.is_empty());
    let detail = state["stack"][0]["lastError"]["detail"].as_str().unwrap();
    assert_eq!(detail.chars().count(), 1024);
    assert!(detail.contains('🦀'));
}

#[test]
fn paused_route_commits_continuation_and_never_repeats_mutation_on_resume() {
    for cold in [false, true] {
        let base = common::tmp_dir("paused-route-resume");
        std::fs::write(base.join("prompt.md"), "Choose").unwrap();
        let run_dir = RunDir::under(&base);
        let plan = PLAN
            .replace(
                "entryStep: choose",
                "mcp: { policies: { t.read: { class: mutation } } }\nentryStep: choose",
            )
            .replace("resumeAt: done", "resumeAt: next")
            .replace(
                "  done:",
                "  next: { kind: task, taskRef: next, onVerified: done }\n  done:",
            );
        let registry = format!("{REGISTRY}\n    mutations: {{ idempotency: intent-log, onUncertain: fail-run }}\n  next: {{ model: coder, prompt: prompt.md }}\n");
        let create = |responses: Vec<String>| {
            Executor::new(
                ExecutionPlan::from_yaml_str(&plan).unwrap(),
                TaskRegistry::from_yaml_str(&registry, &base).unwrap(),
                String::new(),
                SlotMap::new().with("coder", MockModel::scripted(responses)),
                common::ScriptGateway::new(run_dir.artifacts_dir()),
                run_dir.clone(),
            )
            .unwrap()
        };
        let pause = CHOOSE.replace("finish", "pause");
        let mut exec = create(vec![READ.into(), pause, "done".into()]);
        assert!(matches!(
            exec.run_until_blocked().unwrap(),
            RunStatus::Paused { .. }
        ));
        assert_eq!(exec.state().stack[0].node, "next");
        let snapshot: Value =
            serde_json::from_slice(&std::fs::read(run_dir.snapshot_path()).unwrap()).unwrap();
        assert_eq!(snapshot["stack"][0]["node"], "next");
        if cold {
            drop(exec);
            exec = create(vec!["done".into()]);
        }
        assert_eq!(exec.resume().unwrap(), RunStatus::Succeeded);
        let events = common::read_events(run_dir.path());
        assert_eq!(
            events.iter().filter(|e| e["type"] == "model_call").count(),
            3
        );
        assert_eq!(
            events
                .iter()
                .filter(|e| e["type"] == "tool_call_result")
                .count(),
            1
        );
        let choices: Vec<_> = events
            .iter()
            .filter(|e| e["key"] == "routes.pause")
            .collect();
        assert_eq!(choices.len(), 1);
        assert_eq!(choices[0]["resumeAt"], "next");
        std::fs::remove_dir_all(base).unwrap();
    }
}

#[test]
fn explicit_continuation_validation_and_new_field_nulls_fail_closed() {
    use trollm_core::validate::validate_plan;
    for plan in [
        PLAN.replace(", resumeAt: done", ""),
        PLAN.replace("resumeAt: done", "resumeAt: missing"),
        PLAN.replace("status: paused", "status: failed"),
    ] {
        assert!(validate_plan(&ExecutionPlan::from_yaml_str(&plan).unwrap()).is_err());
    }
    for plan in [
        PLAN.replace("resumeAt: done", "resumeAt: null"),
        PLAN.replace("    routes: { finish: done, again: choose, pause: { status: paused, reason: chosen, resumeAt: done } }", "    routes: null\n    onVerified: done"),
        PLAN.replace("promptSupplement: 'Choose at {{ runtime.node }}'", "promptSupplement: null"),
    ] {
        assert!(ExecutionPlan::from_yaml_str(&plan).is_err());
    }
    let plan = ExecutionPlan::from_yaml_str(PLAN).unwrap();
    assert!(ExecutionPlan::from_yaml_str(&serde_yaml::to_string(&plan).unwrap()).is_ok());
}
