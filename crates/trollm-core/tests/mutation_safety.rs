//! Declared/effective mutation safety with fake effects, no remote services.
mod common;

use anyhow::Result;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use trollm_core::{ExecutionPlan, Executor, RunDir, RunStatus, SlotMap, TaskRegistry};
use trollm_mcp::{
    policy::ToolClass, Provenance, ToolCall, ToolDescriptor, ToolGateway, ToolResult,
};
use trollm_models::MockModel;

const PLAN: &str = r#"
schemaVersion: ledger.execution-plan.v1
id: mutation-safety
version: 1
limits: { maxSteps: 10, maxReasonerCalls: 0, maxTokensTotal: 10000, timeoutS: 60, artifactMaxMb: 1 }
onLimit: abort
models: { coder: { ctx: 2048 } }
entryStep: work
nodes:
  work: { kind: task, taskRef: work, onVerified: done }
  done: { kind: end, outcome: succeeded }
"#;
const REGISTRY: &str = r#"
schemaVersion: ledger.task-registry.v1
tasks:
  work:
    model: coder
    prompt: work.md
    tools: [t.write]
    loop: { maxSteps: 4 }
    outputContract:
      outputs: { outcome: { from: model, type: prose } }
"#;
const JOURNAL: &str = "    mutations: { idempotency: intent-log, onUncertain: fail-run }\n";
const CALL: &str = r#"<tool_call>{"server":"t","tool":"write","args":{}}</tool_call>"#;

struct Gateway {
    class: ToolClass,
    effects: Arc<AtomicUsize>,
    run: RunDir,
    lose_response: bool,
}
impl ToolGateway for Gateway {
    fn tool_class(&self, _: &str, _: &str) -> ToolClass {
        self.class
    }
    fn list_tools(&self) -> Result<Vec<ToolDescriptor>> {
        Ok(vec![])
    }
    fn validate_call(&self, _: &ToolCall) -> Result<()> {
        Ok(())
    }
    fn call(&mut self, _: &ToolCall) -> Result<ToolResult> {
        if self.class == ToolClass::Mutation {
            let events = common::read_events(self.run.path());
            assert!(
                events.iter().any(|e| e["type"] == "tool_call_intent"),
                "durable intent must precede fake effect"
            );
        }
        self.effects.fetch_add(1, Ordering::SeqCst);
        anyhow::ensure!(
            !self.lose_response,
            "fake service applied write then lost response"
        );
        Ok(ToolResult {
            content_for_model: serde_json::json!({"ok":true}),
            artifacts: vec![],
            provenance: Provenance::Live,
        })
    }
}
fn setup(
    plan: ExecutionPlan,
    registry: &str,
    class: ToolClass,
    script: Vec<&str>,
    lose_response: bool,
) -> (
    Result<Executor<MockModel, Gateway>>,
    Arc<AtomicUsize>,
    RunDir,
) {
    let base = common::tmp_dir("mutation-safety");
    std::fs::write(base.join("work.md"), "act").unwrap();
    let run = RunDir::under(&base);
    let effects = Arc::new(AtomicUsize::new(0));
    let result = Executor::new(
        plan,
        TaskRegistry::from_yaml_str(registry, &base).unwrap(),
        String::new(),
        SlotMap::new().with("coder", MockModel::scripted(script)),
        Gateway {
            class,
            effects: effects.clone(),
            run: run.clone(),
            lose_response,
        },
        run.clone(),
    );
    (result, effects, run)
}
fn plan(mutation_policy: bool, retry: bool) -> ExecutionPlan {
    let yaml = if retry {
        PLAN.replace(
            "onVerified: done",
            "onVerified: done, onError: { retry: 1, then: done }",
        )
    } else {
        PLAN.to_string()
    };
    let mut plan = ExecutionPlan::from_yaml_str(&yaml).unwrap();
    if mutation_policy {
        plan.mcp
            .policies
            .insert("t.write".into(), serde_json::json!({"class":"mutation"}));
    }
    plan
}
fn journaled() -> String {
    REGISTRY.replace("    loop:", &format!("{JOURNAL}    loop:"))
}

#[test]
fn mutation_grants_require_journaling_for_root_and_runtime_policy() {
    for (policy, gateway) in [(true, ToolClass::Read), (false, ToolClass::Mutation)] {
        let (result, effects, run) =
            setup(plan(policy, false), REGISTRY, gateway, vec![CALL], false);
        assert!(result
            .err()
            .unwrap()
            .to_string()
            .contains("requires mutations.idempotency=intent-log"));
        assert_eq!(effects.load(Ordering::SeqCst), 0);
        assert!(
            !run.path().exists(),
            "reject before creating execution state"
        );
    }
}

#[test]
fn root_policy_applies_to_nested_subflow_grants() {
    for retry in [false, true] {
        let mut root = plan(true, false);
        root.nodes.remove("work");
        root.entry_step = "done".into();
        let mut parent = plan(false, false);
        parent.id = "parent".into();
        parent.nodes.remove("work");
        parent.entry_step = "done".into();
        let mut child = plan(false, retry);
        child.id = "child".into();
        parent.subflows.insert("child".into(), child);
        root.subflows.insert("parent".into(), parent);
        let registry = if retry { journaled() } else { REGISTRY.into() };
        let (result, effects, _) = setup(root, &registry, ToolClass::Read, vec![CALL], false);
        let error = result.err().unwrap().to_string();
        assert!(error.contains("child"), "{error}");
        assert!(
            error.contains(if retry {
                "automatic onError.retry"
            } else {
                "requires mutations.idempotency=intent-log"
            }),
            "{error}"
        );
        assert_eq!(effects.load(Ordering::SeqCst), 0);
    }
}

#[test]
fn mutation_and_intent_log_grants_reject_retry_even_before_dispatch() {
    for (policy, gateway, journal) in [
        (true, ToolClass::Read, false),
        (false, ToolClass::Mutation, true),
        (false, ToolClass::Read, true),
    ] {
        let registry = if journal {
            journaled()
        } else {
            REGISTRY.into()
        };
        let (result, effects, _) = setup(
            plan(policy, true),
            &registry,
            gateway,
            vec!["malformed pre-dispatch output"],
            false,
        );
        assert!(result
            .err()
            .unwrap()
            .to_string()
            .contains("automatic onError.retry"));
        assert_eq!(effects.load(Ordering::SeqCst), 0);
    }
}

#[test]
fn journaled_write_never_repeats_after_response_loss_or_later_failure() {
    for (lose_response, script) in [
        (true, vec![CALL, CALL]),
        (false, vec![CALL]),
        (false, vec![CALL, "<tool_call>invalid</tool_call>"]),
        (false, vec![CALL, ""]),
    ] {
        let registry = if script.last() == Some(&"") {
            journaled().replace("type: prose", "type: list")
        } else {
            journaled()
        };
        let (result, effects, run) = setup(
            plan(false, false),
            &registry,
            ToolClass::Mutation,
            script,
            lose_response,
        );
        let status = result.unwrap().run_until_blocked().unwrap();
        assert!(matches!(status, RunStatus::Failed { .. }), "{status:?}");
        assert_eq!(effects.load(Ordering::SeqCst), 1);
        let events = common::read_events(run.path());
        assert_eq!(
            events
                .iter()
                .filter(|e| e["type"] == "tool_call_intent")
                .count(),
            1
        );
    }
}

#[test]
fn read_only_retry_survives_pre_dispatch_failure() {
    let (result, effects, _) = setup(
        plan(false, true),
        REGISTRY,
        ToolClass::Read,
        vec!["<tool_call>invalid</tool_call>", CALL, "done"],
        false,
    );
    assert_eq!(
        result.unwrap().run_until_blocked().unwrap(),
        RunStatus::Succeeded
    );
    assert_eq!(effects.load(Ordering::SeqCst), 1);
}

#[test]
fn journaled_mutation_dispatches_once_with_durable_intent() {
    let (result, effects, run) = setup(
        plan(true, false),
        &journaled(),
        ToolClass::Mutation,
        vec![CALL, "done"],
        false,
    );
    assert_eq!(
        result.unwrap().run_until_blocked().unwrap(),
        RunStatus::Succeeded
    );
    assert_eq!(effects.load(Ordering::SeqCst), 1);
    let events = common::read_events(run.path());
    let intent = events
        .iter()
        .find(|e| e["type"] == "tool_call_intent")
        .unwrap();
    let result = events
        .iter()
        .find(|e| e["type"] == "tool_call_result")
        .unwrap();
    assert_eq!(result["intentSeq"], intent["seq"]);
}
