//! Crash recovery (§4.4 + §6): a crash between `tool_call_intent` and
//! `tool_call_result` leaves a dangling intent; on resume the task's
//! `onUncertain` policy fires.

mod common;

use common::{read_events, ScriptGateway};
use trollm_core::{ExecutionPlan, Executor, RunDir, RunStatus, SlotMap, TaskRegistry};
use trollm_models::MockModel;

fn plan_yaml() -> String {
    format!(
        r#"
schemaVersion: ledger.execution-plan.v1
id: crash-flow
version: 1
sourceBaseline: {{ sha256: "{}" }}
input: {{ promptTemplate: "go" }}
limits: {{ maxSteps: 10, maxReasonerCalls: 0, maxTokensTotal: 100000, timeoutS: 600, artifactMaxMb: 5 }}
onLimit: abort
models: {{ coder: {{ ctx: 8192 }} }}
mcp: {{ servers: [], policies: {{}} }}
entryStep: act
nodes:
  act: {{ kind: task, taskRef: mutate, onVerified: done }}
  done: {{ kind: end, outcome: succeeded }}
"#,
        "0".repeat(64)
    )
}

fn registry_yaml(on_uncertain: &str) -> String {
    format!(
        r#"
schemaVersion: ledger.task-registry.v1
tasks:
  mutate:
    model: coder
    prompt: m.md
    tools: [t.write]
    loop: {{ maxSteps: 3 }}
    mutations: {{ idempotency: intent-log, onUncertain: {on_uncertain} }}
    outputContract:
      outputs: {{ outcome: {{ from: model, type: prose }} }}
"#
    )
}

const FENCE: &str = r#"<tool_call>{"server":"t","tool":"write","args":{"id":7}}</tool_call>"#;

fn write_prompt(base: &std::path::Path) {
    std::fs::write(base.join("m.md"), "mutate now").unwrap();
}

/// First executor crashes mid-call: the panic simulates process death after
/// the intent hit the log but before the result was written.
fn run_crashing_executor(run_dir: &RunDir, base: &std::path::Path, on_uncertain: &str) {
    let gateway = ScriptGateway {
        crash_on: Some("t.write".to_string()),
        ..Default::default()
    };
    let mut exec = Executor::new(
        ExecutionPlan::from_yaml_str(&plan_yaml()).unwrap(),
        TaskRegistry::from_yaml_str(&registry_yaml(on_uncertain), base).unwrap(),
        String::new(),
        SlotMap::new().with("coder", MockModel::scripted([FENCE, "unused"])),
        gateway,
        run_dir.clone(),
    )
    .unwrap();
    let hit = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = exec.run_until_blocked();
    }));
    assert!(hit.is_err(), "the simulated crash must propagate");
    // exec drops here, mid-task, exactly like a process crash.
}

#[test]
fn crash_between_intent_and_result_triggers_refetch_state() {
    // the simulated panic is expected; keep test output readable, but let real
    // assertion failures print
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if !info.to_string().contains("simulated crash") {
            default_hook(info);
        }
    }));

    let base = common::tmp_dir("crash-refetch");
    write_prompt(&base);
    let run_dir = RunDir::under(&base);

    run_crashing_executor(&run_dir, &base, "refetch-state");

    // On disk: an intent with no result — the exact crash signature of §6.
    let events = read_events(run_dir.path());
    let intents: Vec<_> = events
        .iter()
        .filter(|e| e["type"] == "tool_call_intent")
        .collect();
    assert_eq!(intents.len(), 1);
    assert!(!events.iter().any(|e| e["type"] == "tool_call_result"));

    // Resume from disk with a healthy gateway; the task re-executes from its
    // node boundary (at most one node re-executed, §4.4).
    let mut exec = Executor::new(
        ExecutionPlan::from_yaml_str(&plan_yaml()).unwrap(),
        TaskRegistry::from_yaml_str(&registry_yaml("refetch-state"), &base).unwrap(),
        String::new(),
        SlotMap::new().with("coder", MockModel::scripted([FENCE, "write completed"])),
        ScriptGateway::default(),
        run_dir.clone(),
    )
    .unwrap();
    let status = exec.resume().unwrap();
    assert_eq!(status, RunStatus::Succeeded);

    let events = read_events(run_dir.path());
    // onUncertain=refetch-state fired: the dangling intent is closed by a
    // skipped-result note (the state refetch itself is the gateway's concern).
    let skips: Vec<_> = events
        .iter()
        .filter(|e| e["type"] == "tool_call_result" && e["skipped"] == true)
        .collect();
    assert_eq!(skips.len(), 1, "dangling intent marked skipped");
    assert_eq!(skips[0]["intentSeq"], intents[0]["seq"]);
    assert!(skips[0]["note"].as_str().unwrap().contains("refetch-state"));
    // the re-executed task journaled a fresh intent + real result
    let intents_after: Vec<_> = events
        .iter()
        .filter(|e| e["type"] == "tool_call_intent")
        .collect();
    assert_eq!(intents_after.len(), 2);
    let real_results: Vec<_> = events
        .iter()
        .filter(|e| e["type"] == "tool_call_result" && e["skipped"] != true)
        .collect();
    assert_eq!(real_results.len(), 1);
    assert_eq!(events.last().unwrap()["outcome"], "succeeded");
}

#[test]
fn crash_between_intent_and_result_with_fail_run_policy_fails() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if !info.to_string().contains("simulated crash") {
            default_hook(info);
        }
    }));

    let base = common::tmp_dir("crash-failrun");
    write_prompt(&base);
    let run_dir = RunDir::under(&base);

    run_crashing_executor(&run_dir, &base, "fail-run");

    let mut exec = Executor::new(
        ExecutionPlan::from_yaml_str(&plan_yaml()).unwrap(),
        TaskRegistry::from_yaml_str(&registry_yaml("fail-run"), &base).unwrap(),
        String::new(),
        SlotMap::new().with("coder", MockModel::scripted(["unreached"])),
        ScriptGateway::default(),
        run_dir.clone(),
    )
    .unwrap();
    let status = exec.resume().unwrap();
    match status {
        RunStatus::Failed { reason } => {
            assert!(reason.contains("uncertain mutation"), "{reason}")
        }
        other => panic!("expected Failed, got {other:?}"),
    }
    let events = read_events(run_dir.path());
    assert_eq!(events.last().unwrap()["type"], "run_ended");
    assert_eq!(events.last().unwrap()["outcome"], "failed");
}

/// Kill generation after one or several completed writes, before a transition.
struct CrashAfterWrites {
    remaining: usize,
}
impl trollm_models::ModelSlot for CrashAfterWrites {
    fn generate(
        &mut self,
        _: trollm_models::GenerateRequest,
    ) -> anyhow::Result<trollm_models::GenerateResponse> {
        if self.remaining == 0 {
            panic!("simulated crash after result");
        }
        self.remaining -= 1;
        Ok(trollm_models::GenerateResponse {
            text: FENCE.into(),
            tokens_in: 1,
            tokens_out: 1,
            finish: trollm_models::FinishReason::Stop,
        })
    }
}

struct CountingGateway(std::sync::Arc<std::sync::atomic::AtomicUsize>);
impl trollm_mcp::ToolGateway for CountingGateway {
    fn list_tools(&self) -> anyhow::Result<Vec<trollm_mcp::ToolDescriptor>> {
        Ok(vec![])
    }
    fn validate_call(&self, call: &trollm_mcp::ToolCall) -> anyhow::Result<()> {
        anyhow::ensure!(
            call.server == "t" && call.tool == "write",
            "unsupported test tool"
        );
        Ok(())
    }
    fn call(&mut self, _: &trollm_mcp::ToolCall) -> anyhow::Result<trollm_mcp::ToolResult> {
        self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(trollm_mcp::ToolResult {
            content_for_model: serde_json::json!({"ok": true}),
            artifacts: vec![],
            provenance: trollm_mcp::Provenance::Live,
        })
    }
}

#[test]
fn completed_mutations_after_snapshot_fail_closed_before_replay() {
    for (writes, use_resume) in [(1, true), (2, true), (1, false)] {
        let base = common::tmp_dir("completed-replay");
        write_prompt(&base);
        let run_dir = RunDir::under(&base);
        let effects = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut exec = Executor::new(
            ExecutionPlan::from_yaml_str(&plan_yaml()).unwrap(),
            TaskRegistry::from_yaml_str(&registry_yaml("fail-run"), &base).unwrap(),
            String::new(),
            SlotMap::new().with("coder", CrashAfterWrites { remaining: writes }),
            CountingGateway(effects.clone()),
            run_dir.clone(),
        )
        .unwrap();
        assert!(std::panic::catch_unwind(
            std::panic::AssertUnwindSafe(|| exec.run_until_blocked())
        )
        .is_err());
        drop(exec);
        let mut restored = Executor::new(
            ExecutionPlan::from_yaml_str(&plan_yaml()).unwrap(),
            TaskRegistry::from_yaml_str(&registry_yaml("fail-run"), &base).unwrap(),
            String::new(),
            SlotMap::new().with("coder", MockModel::scripted([FENCE, "done"])),
            CountingGateway(effects.clone()),
            run_dir.clone(),
        )
        .unwrap();
        let status = if use_resume {
            restored.resume()
        } else {
            restored.run_until_blocked()
        }
        .unwrap();
        assert!(matches!(status, RunStatus::Failed { reason } if reason.contains("unsafe replay")));
        assert_eq!(effects.load(std::sync::atomic::Ordering::SeqCst), writes);
        let events = read_events(run_dir.path());
        assert_eq!(
            events
                .iter()
                .filter(|e| e["type"] == "tool_call_intent")
                .count(),
            writes
        );
        assert_eq!(
            events
                .iter()
                .filter(|e| e["type"] == "tool_call_result")
                .count(),
            writes
        );
    }
}

#[test]
fn committed_mutation_does_not_block_legitimate_revisit_after_pause() {
    let base = common::tmp_dir("completed-revisit");
    write_prompt(&base);
    let run_dir = RunDir::under(&base);
    let yaml = plan_yaml().replace(
        "onVerified: done",
        "onVerified: { status: paused, reason: revisit }",
    );
    let mut exec = Executor::new(
        ExecutionPlan::from_yaml_str(&yaml).unwrap(),
        TaskRegistry::from_yaml_str(&registry_yaml("fail-run"), &base).unwrap(),
        String::new(),
        SlotMap::new().with("coder", MockModel::scripted([FENCE, "done"])),
        ScriptGateway::default(),
        run_dir.clone(),
    )
    .unwrap();
    assert!(matches!(
        exec.run_until_blocked().unwrap(),
        RunStatus::Paused { .. }
    ));
    drop(exec);
    let mut restored = Executor::new(
        ExecutionPlan::from_yaml_str(&yaml).unwrap(),
        TaskRegistry::from_yaml_str(&registry_yaml("fail-run"), &base).unwrap(),
        String::new(),
        SlotMap::new().with("coder", MockModel::scripted([FENCE, "done"])),
        ScriptGateway::default(),
        run_dir.clone(),
    )
    .unwrap();
    assert!(matches!(
        restored.resume().unwrap(),
        RunStatus::Paused { .. }
    ));
    assert_eq!(
        read_events(run_dir.path())
            .iter()
            .filter(|e| e["type"] == "tool_call_intent")
            .count(),
        2
    );
}
