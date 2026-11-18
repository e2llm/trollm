//! Explicit authored calls with real local tools and scripted gateway effects.
//! No model weights, remote MCP services, or device execution are involved.

mod common;

use anyhow::{ensure, Result};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc, Mutex,
};
use std::time::Duration;
use trollm_core::artifacts::{self, ArtifactMeta};
use trollm_core::exec::ExecutionControl;
use trollm_core::state::MutationOutcome;
use trollm_core::{
    ExecutionPlan, Executor, RunDir, RunStatus, SlotMap, TaskExecution, TaskRegistry,
};
use trollm_mcp::{
    policy::ToolClass, ArtifactSchema, McpGateway, PolicySet, Provenance, ToolCall, ToolDescriptor,
    ToolGateway, ToolResult,
};
use trollm_models::MockModel;

const PLAN: &str = r#"
schemaVersion: ledger.execution-plan.v1
id: direct-tool-test
version: 1
limits: { maxSteps: 8, maxReasonerCalls: 0, maxTokensTotal: 0, timeoutS: 60, artifactMaxMb: 1 }
onLimit: abort
models: {}
entryStep: work
nodes:
  work: { kind: task, taskRef: work, onVerified: done }
  done: { kind: end, outcome: succeeded }
"#;

const REGISTRY: &str = r#"
schemaVersion: ledger.task-registry.v1
tasks:
  work:
    execution: tool
    completion: tool-result
    tools: [fixture.operate]
    toolArguments:
      fixture.operate:
        modelFields: []
        fromInput: { id: id }
        fixed: { options: { enabled: true, labels: [first, second] } }
    outputContract:
      outputs:
        original: { fromArtifact: observed, format: json, type: object }
      requireArtifactComplete: [observed]
"#;

const MODEL_CALL: &str =
    r#"<tool_call>{"server":"fixture","tool":"operate","args":{}}</tool_call>"#;

fn argument_schema() -> Value {
    json!({
        "type": "object", "additionalProperties": false,
        "required": ["id", "options"],
        "properties": {
            "id": {"type": "integer", "minimum": 1},
            "options": {
                "type": "object", "additionalProperties": false,
                "required": ["enabled", "labels"],
                "properties": {
                    "enabled": {"type": "boolean"},
                    "labels": {"type": "array", "items": {"type": "string"}}
                }
            }
        }
    })
}

#[derive(Clone, Copy)]
enum Behavior {
    Artifact,
    Fail,
    FirstArtifactOnly,
}

#[derive(Default)]
struct CancelControl(AtomicBool);

impl ExecutionControl for CancelControl {
    fn checkpoint(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }

    fn wait_backoff(&self, _: Duration) -> bool {
        self.checkpoint()
    }
}

struct Gateway {
    run: RunDir,
    class: ToolClass,
    behavior: Behavior,
    calls: Arc<Mutex<Vec<ToolCall>>>,
    validations: Arc<AtomicUsize>,
    cancel_on_return: Option<Arc<CancelControl>>,
}

impl ToolGateway for Gateway {
    fn validate_direct_call(&self, _: &ToolCall) -> Result<()> {
        // This fixture implements ordinary calls and has no selected preparer.
        Ok(())
    }

    fn tool_class(&self, _: &str, _: &str) -> ToolClass {
        self.class
    }

    fn list_tools(&self) -> Result<Vec<ToolDescriptor>> {
        Ok(vec![ToolDescriptor {
            server: "fixture".into(),
            name: "operate".into(),
            input_schema: argument_schema(),
        }])
    }

    fn argument_constraints(&self, _: &str, _: &str) -> Option<Value> {
        Some(argument_schema())
    }

    fn validate_call(&self, call: &ToolCall) -> Result<()> {
        self.validations.fetch_add(1, Ordering::SeqCst);
        // The test exercises validation before intent, including schema failure.
        let events = common::read_events(self.run.path());
        assert_eq!(event_count(&events, "tool_call_intent"), 0);
        ensure!(
            call.server == "fixture" && call.tool == "operate",
            "unknown tool"
        );
        ArtifactSchema::compile(&argument_schema())?.validate(&call.args, 4096)
    }

    fn call(&mut self, call: &ToolCall) -> Result<ToolResult> {
        let events = common::read_events(self.run.path());
        if self.class == ToolClass::Mutation {
            let intent = events
                .iter()
                .find(|event| event["type"] == "tool_call_intent")
                .expect("mutation intent must already be in the journal");
            assert_eq!(intent["args"], call.args);
            assert_eq!(event_count(&events, "tool_call_result"), 0);
        }
        let ordinal = {
            let mut calls = self.calls.lock().unwrap();
            calls.push(call.clone());
            calls.len()
        };
        // A mutation in this branch has taken effect before the response is lost.
        ensure!(
            !matches!(self.behavior, Behavior::Fail),
            "scripted response loss"
        );
        let content = json!({"received": call.args, "generation": ordinal});
        let mut paths = vec![];
        if !matches!(self.behavior, Behavior::FirstArtifactOnly) || ordinal == 1 {
            let written = artifacts::write_atomic(
                &self.run.artifacts_dir(),
                "observed.json",
                &serde_json::to_vec(&content)?,
            )?;
            artifacts::write_meta(
                &written.path,
                &ArtifactMeta {
                    fetched_at: 0,
                    complete: true,
                },
            )?;
            paths.push(written.path);
        }
        if let Some(control) = &self.cancel_on_return {
            control.0.store(true, Ordering::SeqCst);
        }
        Ok(ToolResult {
            content_for_model: content,
            artifacts: paths,
            provenance: Provenance::Live,
        })
    }
}

struct Fixture {
    base: PathBuf,
    run: RunDir,
    calls: Arc<Mutex<Vec<ToolCall>>>,
    validations: Arc<AtomicUsize>,
}

impl Fixture {
    fn new() -> Self {
        let base = common::tmp_dir("direct-tool-execution");
        Self {
            run: RunDir::under(&base),
            base,
            calls: Arc::default(),
            validations: Arc::default(),
        }
    }

    fn gateway(&self, class: ToolClass, behavior: Behavior) -> Gateway {
        Gateway {
            run: self.run.clone(),
            class,
            behavior,
            calls: self.calls.clone(),
            validations: self.validations.clone(),
            cancel_on_return: None,
        }
    }

    fn executor(
        &self,
        plan: &str,
        registry: &str,
        input: Value,
        gateway: Gateway,
    ) -> Result<Executor<MockModel, Gateway>> {
        Executor::new(
            ExecutionPlan::from_yaml_str(plan)?,
            TaskRegistry::from_yaml_str(registry, &self.base)?,
            input.to_string(),
            SlotMap::<MockModel>::new(),
            gateway,
            self.run.clone(),
        )
    }

    fn events(&self) -> Vec<Value> {
        common::read_events(self.run.path())
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.base);
    }
}

fn event_count(events: &[Value], kind: &str) -> usize {
    events.iter().filter(|event| event["type"] == kind).count()
}

fn assert_direct_accounting<G: ToolGateway>(executor: &Executor<MockModel, G>, steps: u32) {
    assert_eq!(executor.state().counters.steps, steps);
    assert_eq!(executor.state().counters.tokens, 0);
    assert_eq!(executor.state().counters.reasoner_calls, 0);
    assert_eq!(executor.state().stack[0].counters.steps, steps);
    let events = common::read_events(executor.run_dir().path());
    assert_eq!(event_count(&events, "direct_tool_step"), steps as usize);
    assert_eq!(event_count(&events, "model_call"), 0);
    for event in events
        .iter()
        .filter(|event| event["type"] == "direct_tool_step")
    {
        assert_eq!(event["execution"], "tool");
        assert!(event["attemptContext"]["attempt"].as_u64().is_some());
        assert!(event.get("model").is_none());
    }
}

fn journaled_registry() -> String {
    format!("{REGISTRY}\n    mutations: {{ idempotency: intent-log, onUncertain: fail-run }}\n")
}

#[test]
fn real_local_example_completes_with_no_slots_or_model_budget() {
    let fixture = Fixture::new();
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let plan =
        ExecutionPlan::from_yaml_file(&root.join("flows/examples/direct-calculation.flow.yaml"))
            .unwrap();
    let registry =
        TaskRegistry::from_yaml_file(&root.join("flows/tasks/direct-calculation.tasks.yaml"))
            .unwrap();
    assert!(plan.models.is_empty());
    assert_eq!(plan.limits.max_tokens_total, 0);
    assert!(registry
        .tasks
        .values()
        .all(|task| task.execution == TaskExecution::Tool
            && task.model_slot().is_none()
            && task.prompt_path().is_none()));
    let exports = plan.exports.clone().unwrap();
    let policies =
        PolicySet::from_yaml_value(&serde_yaml::to_value(&plan.mcp.policies).unwrap()).unwrap();
    let gateway = McpGateway::new(
        vec![],
        policies,
        fixture.base.join("cache"),
        fixture.run.artifacts_dir(),
        None,
    )
    .unwrap();
    let mut executor = Executor::new(
        plan,
        registry,
        json!({"left":"39.99", "right":"40.10", "budget":"80"}).to_string(),
        SlotMap::<MockModel>::new(),
        gateway,
        fixture.run.clone(),
    )
    .unwrap();

    assert_eq!(executor.run_until_blocked().unwrap(), RunStatus::Succeeded);
    let vars = &executor.state().stack[0].vars;
    let sum = json!({"format":"trollm.local.calculate.v1", "operation":"add",
        "left":"39.99", "right":"40.10", "result":"80.09"});
    let comparison = json!({"format":"trollm.local.calculate.v1", "operation":"compare",
        "left":"80.09", "right":"80", "result":"greater"});
    assert_eq!(exports["sum"].from, "sum.outputs.calculation");
    assert_eq!(exports["comparison"].from, "compare.outputs.calculation");
    assert_eq!(vars["sum"]["outputs"]["calculation"], sum);
    assert_eq!(vars["compare"]["outputs"]["calculation"], comparison);
    let artifact = fixture.run.artifacts_dir().join("calculation.json");
    assert_eq!(
        serde_json::from_slice::<Value>(&std::fs::read(&artifact).unwrap()).unwrap(),
        comparison
    );
    assert!(artifacts::is_complete(&artifact));
    let events = fixture.events();
    assert_eq!(event_count(&events, "tool_call_result"), 2);
    assert!(events
        .iter()
        .filter(|event| event["type"] == "tool_call_result")
        .all(|event| event["provenance"] == "Local"));
    assert_eq!(event_count(&events, "tool_call_intent"), 0);
    assert_direct_accounting(&executor, 2);
}

#[test]
fn explicit_direct_and_legacy_model_calls_preserve_arguments_and_artifact_bytes() {
    let direct = Fixture::new();
    let legacy = Fixture::new();
    let input = json!({"id":7});
    let mut direct_executor = direct
        .executor(
            PLAN,
            REGISTRY,
            input.clone(),
            direct.gateway(ToolClass::Read, Behavior::Artifact),
        )
        .unwrap();
    std::fs::write(
        legacy.base.join("work.md"),
        "Call the granted tool once with args:{}.",
    )
    .unwrap();
    let model_plan = PLAN
        .replace("models: {}", "models: { coder: { ctx: 2048 } }")
        .replace("maxTokensTotal: 0", "maxTokensTotal: 10000");
    let model_registry = REGISTRY.replace("execution: tool", "model: coder\n    prompt: work.md");
    let mut model_executor = Executor::new(
        ExecutionPlan::from_yaml_str(&model_plan).unwrap(),
        TaskRegistry::from_yaml_str(&model_registry, &legacy.base).unwrap(),
        input.to_string(),
        SlotMap::new().with("coder", MockModel::scripted([MODEL_CALL])),
        legacy.gateway(ToolClass::Read, Behavior::Artifact),
        legacy.run.clone(),
    )
    .unwrap();
    assert_eq!(
        direct_executor.run_until_blocked().unwrap(),
        RunStatus::Succeeded
    );
    assert_eq!(
        model_executor.run_until_blocked().unwrap(),
        RunStatus::Succeeded
    );
    let direct_calls = direct.calls.lock().unwrap();
    let model_calls = legacy.calls.lock().unwrap();
    assert_eq!(direct_calls.len(), 1);
    assert_eq!(model_calls.len(), 1);
    assert_eq!(
        serde_json::to_value(&*direct_calls).unwrap(),
        serde_json::to_value(&*model_calls).unwrap()
    );
    assert_eq!(
        direct_calls[0].args,
        json!({"id":7,"options":{"enabled":true,"labels":["first","second"]}})
    );
    assert_eq!(
        std::fs::read(direct.run.artifacts_dir().join("observed.json")).unwrap(),
        std::fs::read(legacy.run.artifacts_dir().join("observed.json")).unwrap()
    );
    assert_eq!(
        direct_executor.state().stack[0].node_outputs,
        model_executor.state().stack[0].node_outputs
    );
    assert_direct_accounting(&direct_executor, 1);
    assert_eq!(event_count(&legacy.events(), "model_call"), 1);
    assert_eq!(event_count(&legacy.events(), "direct_tool_step"), 0);
    assert_eq!(model_executor.state().counters.steps, 1);
    assert!(model_executor.state().counters.tokens > 0);
}

#[test]
fn missing_binding_and_schema_mismatch_fail_before_mutation_intent_or_effect() {
    for (input, validations) in [(json!({}), 0), (json!({"id":"7"}), 1)] {
        let fixture = Fixture::new();
        let mut executor = fixture
            .executor(
                PLAN,
                &journaled_registry(),
                input,
                fixture.gateway(ToolClass::Mutation, Behavior::Artifact),
            )
            .unwrap();
        assert!(matches!(
            executor.run_until_blocked().unwrap(),
            RunStatus::Failed { .. }
        ));
        assert!(fixture.calls.lock().unwrap().is_empty());
        assert_eq!(fixture.validations.load(Ordering::SeqCst), validations);
        let events = fixture.events();
        assert_eq!(event_count(&events, "tool_call_intent"), 0);
        assert_eq!(event_count(&events, "tool_call_result"), 0);
        assert!(!fixture.run.artifacts_dir().join("observed.json").exists());
        assert_direct_accounting(&executor, 1);
    }
}

#[test]
fn success_and_error_cycles_stop_at_the_execution_step_limit() {
    for (behavior, work) in [
        (Behavior::Artifact, "onVerified: work"),
        (
            Behavior::Fail,
            "onVerified: done, onError: { retry: 0, then: work }",
        ),
    ] {
        let fixture = Fixture::new();
        let plan = PLAN
            .replace("maxSteps: 8", "maxSteps: 3")
            .replace("onVerified: done", work);
        let mut executor = fixture
            .executor(
                &plan,
                REGISTRY,
                json!({"id":7}),
                fixture.gateway(ToolClass::Read, behavior),
            )
            .unwrap();
        assert!(
            matches!(executor.run_until_blocked().unwrap(), RunStatus::Failed { reason }
            if reason.contains("budget-exhausted"))
        );
        assert_eq!(fixture.calls.lock().unwrap().len(), 3);
        assert_direct_accounting(&executor, 3);
        let events = fixture.events();
        let exhausted = events
            .iter()
            .find(|event| event["type"] == "budget_exhausted")
            .unwrap();
        assert_eq!(exhausted["budget"], "maxSteps");
        assert_eq!(exhausted["value"], 3);
    }
}

#[test]
fn retries_charge_direct_steps_even_when_binding_fails_before_dispatch() {
    for (input, calls) in [(json!({}), 0), (json!({"id":7}), 2)] {
        let fixture = Fixture::new();
        let plan = PLAN.replace("maxSteps: 8", "maxSteps: 2").replace(
            "onVerified: done",
            "onVerified: done, onError: { retry: 10, backoffS: 0, then: done }",
        );
        let mut executor = fixture
            .executor(
                &plan,
                REGISTRY,
                input,
                fixture.gateway(ToolClass::Read, Behavior::Fail),
            )
            .unwrap();
        assert!(
            matches!(executor.run_until_blocked().unwrap(), RunStatus::Failed { reason }
            if reason.contains("budget-exhausted"))
        );
        assert_eq!(fixture.calls.lock().unwrap().len(), calls);
        assert_direct_accounting(&executor, 2);
        assert!(!fixture
            .events()
            .iter()
            .any(|event| event["type"] == "transition" && event["to"] == "done"));
    }
}

#[test]
fn previous_complete_artifact_and_output_alias_cannot_satisfy_a_new_direct_call() {
    let fixture = Fixture::new();
    let plan = PLAN.replace("onVerified: done", "onVerified: work");
    let mut executor = fixture
        .executor(
            &plan,
            REGISTRY,
            json!({"id":7}),
            fixture.gateway(ToolClass::Read, Behavior::FirstArtifactOnly),
        )
        .unwrap();
    assert!(matches!(
        executor.run_until_blocked().unwrap(),
        RunStatus::Failed { .. }
    ));
    assert_eq!(fixture.calls.lock().unwrap().len(), 2);
    assert_direct_accounting(&executor, 2);
    let frame = &executor.state().stack[0];
    assert!(frame.vars.get("work").is_none());
    assert!(!frame.node_outputs.contains_key("work"));
    assert!(frame
        .last_error
        .as_ref()
        .unwrap()
        .detail
        .contains("current-call artifact"));
    // Retained original evidence may still exist on disk, but cannot verify this attempt.
    assert!(artifacts::is_complete(
        &fixture.run.artifacts_dir().join("observed.json")
    ));
    let events = fixture.events();
    assert_eq!(event_count(&events, "tool_call_result"), 2);
    assert_eq!(
        events
            .iter()
            .filter(|event| event["type"] == "transition" && event["key"] == "onVerified")
            .count(),
        1
    );
}

#[test]
fn ordinary_mutation_keeps_intent_before_effect_and_links_returned_result() {
    let fixture = Fixture::new();
    let mut executor = fixture
        .executor(
            PLAN,
            &journaled_registry(),
            json!({"id":7}),
            fixture.gateway(ToolClass::Mutation, Behavior::Artifact),
        )
        .unwrap();
    assert_eq!(executor.run_until_blocked().unwrap(), RunStatus::Succeeded);
    assert_eq!(fixture.calls.lock().unwrap().len(), 1);
    assert_direct_accounting(&executor, 1);
    let events = fixture.events();
    let intent = events
        .iter()
        .find(|event| event["type"] == "tool_call_intent")
        .unwrap();
    let result = events
        .iter()
        .find(|event| event["type"] == "tool_call_result")
        .unwrap();
    assert_eq!(result["intentSeq"], intent["seq"]);
    assert_eq!(result["callContext"], intent["callContext"]);
    assert!(intent["seq"].as_u64().unwrap() < result["seq"].as_u64().unwrap());
    assert_eq!(
        executor.state().stack[0].vars["work"]["outputs"]["original"]["received"]["id"],
        7
    );
}

#[test]
fn ordinary_mutation_retry_is_rejected_before_execution_state_is_created() {
    let fixture = Fixture::new();
    let plan = PLAN.replace(
        "onVerified: done",
        "onVerified: done, onError: { retry: 1, then: done }",
    );
    let error = fixture
        .executor(
            &plan,
            &journaled_registry(),
            json!({"id":7}),
            fixture.gateway(ToolClass::Mutation, Behavior::Fail),
        )
        .err()
        .expect("mutation retries must be rejected");
    assert!(
        error.to_string().contains("automatic onError.retry"),
        "{error:#}"
    );
    assert!(fixture.calls.lock().unwrap().is_empty());
    assert!(!fixture.run.path().exists());
}

#[test]
fn unknown_mutation_records_authored_pause_and_cold_resume_refuses_replay() {
    let fixture = Fixture::new();
    let plan = PLAN.replace("onVerified: done",
        "onVerified: done, onError: { retry: 0, then: { status: paused, reason: Observe before continuing, resumeAt: work } }");
    let mut executor = fixture
        .executor(
            &plan,
            &journaled_registry(),
            json!({"id":7}),
            fixture.gateway(ToolClass::Mutation, Behavior::Fail),
        )
        .unwrap();
    assert_eq!(
        executor.run_until_blocked().unwrap(),
        RunStatus::Paused {
            reason: "Observe before continuing".into(),
        }
    );
    assert_eq!(
        fixture.calls.lock().unwrap().len(),
        1,
        "the fake write already took effect"
    );
    assert_direct_accounting(&executor, 1);
    assert_eq!(
        executor.state().stack[0]
            .last_error
            .as_ref()
            .unwrap()
            .mutation_outcome,
        MutationOutcome::Unknown
    );
    drop(executor);
    // Active-worker authored recovery remains available. A new worker must
    // honor the unresolved durable intent rather than replay this direct write.
    for _ in 0..2 {
        let mut restored = fixture
            .executor(
                &plan,
                &journaled_registry(),
                json!({"id":7}),
                fixture.gateway(ToolClass::Mutation, Behavior::Artifact),
            )
            .unwrap();
        assert!(
            matches!(restored.resume().unwrap(), RunStatus::Failed { reason }
            if reason.contains("uncertain mutation"))
        );
        assert_direct_accounting(&restored, 1);
    }
    assert_eq!(fixture.calls.lock().unwrap().len(), 1);
    let events = fixture.events();
    assert_eq!(event_count(&events, "tool_call_intent"), 1);
    assert_eq!(event_count(&events, "tool_call_result"), 0);
    assert_eq!(
        events
            .iter()
            .filter(|event| event["type"] == "transition" && event["key"] == "onError")
            .count(),
        1
    );
    assert!(!events
        .iter()
        .any(|event| event["type"] == "run_ended" && event["outcome"] == "succeeded"));
}

#[test]
fn cancellation_during_mutation_retains_returned_result_and_artifact_before_stopping() {
    let fixture = Fixture::new();
    let control = Arc::new(CancelControl::default());
    let mut gateway = fixture.gateway(ToolClass::Mutation, Behavior::Artifact);
    gateway.cancel_on_return = Some(control.clone());
    let mut executor = fixture
        .executor(PLAN, &journaled_registry(), json!({"id":7}), gateway)
        .unwrap();
    executor.set_control(control);
    assert_eq!(
        executor.run_until_blocked().unwrap(),
        RunStatus::Failed {
            reason: "cancelled".into()
        }
    );
    assert_eq!(fixture.calls.lock().unwrap().len(), 1);
    assert_direct_accounting(&executor, 1);
    let events = fixture.events();
    let intent = events
        .iter()
        .find(|event| event["type"] == "tool_call_intent")
        .unwrap();
    let result = events
        .iter()
        .find(|event| event["type"] == "tool_call_result")
        .unwrap();
    let artifact = events
        .iter()
        .find(|event| event["type"] == "artifact_written")
        .unwrap();
    let terminal = events.last().unwrap();
    assert_eq!(result["intentSeq"], intent["seq"]);
    assert_eq!(terminal["outcome"], "cancelled");
    assert!(result["seq"].as_u64().unwrap() < terminal["seq"].as_u64().unwrap());
    assert!(artifact["seq"].as_u64().unwrap() < terminal["seq"].as_u64().unwrap());
    assert!(artifacts::is_complete(
        &fixture.run.artifacts_dir().join("observed.json")
    ));
    assert!(!events.iter().any(|event| event["type"] == "transition"));
    let saved: trollm_core::RunState =
        serde_json::from_slice(&std::fs::read(fixture.run.snapshot_path()).unwrap()).unwrap();
    assert_eq!(saved.terminal_reason.as_deref(), Some("cancelled"));
}

#[test]
fn direct_registry_rejects_model_configuration_and_roundtrips_without_placeholders() {
    let fixture = Fixture::new();
    let registry = TaskRegistry::from_yaml_str(REGISTRY, &fixture.base).unwrap();
    let encoded = serde_yaml::to_string(&registry).unwrap();
    let decoded = TaskRegistry::from_yaml_str(&encoded, &fixture.base).unwrap();
    let task = &decoded.tasks["work"];
    assert_eq!(task.execution, TaskExecution::Tool);
    assert!(task.model.is_none() && task.prompt.is_none());
    let value = serde_json::to_value(task).unwrap();
    for field in [
        "model",
        "prompt",
        "maxOutputTokens",
        "generationTimeoutS",
        "loop",
    ] {
        assert!(value.get(field).is_none(), "unexpected serialized {field}");
    }
    for field in [
        "model: coder",
        "model: null",
        "prompt: work.md",
        "prompt: null",
        "maxOutputTokens: 10",
        "maxOutputTokens: null",
        "generationTimeoutS: 10",
        "generationTimeoutS: null",
        "loop: { maxSteps: 1 }",
        "loop: null",
        "mutations: null",
    ] {
        let invalid = format!("{REGISTRY}\n    {field}\n");
        assert!(
            TaskRegistry::from_yaml_str(&invalid, &fixture.base).is_err(),
            "accepted direct field {field}"
        );
        let tagged = invalid.replace("execution: tool", "execution: !tool");
        assert!(
            TaskRegistry::from_yaml_str(&tagged, &fixture.base).is_err(),
            "accepted tagged direct field {field}"
        );
    }
    let legacy = REGISTRY.replace("execution: tool", "model: coder\n    prompt: work.md");
    for field in [
        "maxOutputTokens: null",
        "generationTimeoutS: null",
        "loop: null",
    ] {
        TaskRegistry::from_yaml_str(&format!("{legacy}\n    {field}\n"), &fixture.base).unwrap();
    }
    let legacy = TaskRegistry::from_yaml_str(&legacy, &fixture.base).unwrap();
    assert_eq!(legacy.tasks["work"].execution, TaskExecution::Model);
    assert!(serde_json::to_value(&legacy.tasks["work"])
        .unwrap()
        .get("execution")
        .is_none());
}
