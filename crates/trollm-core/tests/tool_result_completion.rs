//! Scripted completion-mode checks: no real models or MCP transport.
mod common;

use anyhow::{ensure, Result};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use trollm_core::artifacts::{self, ArtifactMeta};
use trollm_core::registry::TaskCompletion;
use trollm_core::{ExecutionPlan, Executor, RunDir, RunStatus, SlotMap, TaskRegistry};
use trollm_mcp::{Provenance, ToolCall, ToolDescriptor, ToolGateway, ToolResult};
use trollm_models::{GenerateRequest, GenerateResponse, MockModel, ModelSlot};

const PLAN: &str = r#"
schemaVersion: ledger.execution-plan.v1
id: tool-result
version: 1
limits: { maxSteps: 4, maxReasonerCalls: 0, maxTokensTotal: 10000, timeoutS: 60, artifactMaxMb: 1 }
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
    completion: tool-result
    tools: [t.write]
    mutations: { idempotency: intent-log, onUncertain: fail-run }
"#;
const CALL: &str = r#"<tool_call>{"server":"t","tool":"write","args":{"id":"7"}}</tool_call>"#;

struct RecordingModel {
    model: MockModel,
    prompts: Arc<Mutex<Vec<String>>>,
}
impl ModelSlot for RecordingModel {
    fn generate(&mut self, request: GenerateRequest) -> Result<GenerateResponse> {
        self.prompts.lock().unwrap().push(request.prompt.clone());
        self.model.generate(request)
    }
}

#[derive(Clone, Copy, Default)]
enum Behavior {
    #[default]
    Success,
    Error,
    CompleteArtifact,
    IncompleteArtifact,
}
struct Gateway {
    run: RunDir,
    calls: Arc<Mutex<Vec<ToolCall>>>,
    behavior: Behavior,
}
impl ToolGateway for Gateway {
    fn list_tools(&self) -> Result<Vec<ToolDescriptor>> {
        Ok(vec![])
    }
    fn validate_call(&self, call: &ToolCall) -> Result<()> {
        // Validation must happen before any mutation intent.
        assert!(!common::read_events(self.run.path())
            .iter()
            .any(|event| event["type"] == "tool_call_intent"));
        ensure!(call.args["id"].is_string(), "id must be a string");
        Ok(())
    }
    fn call(&mut self, call: &ToolCall) -> Result<ToolResult> {
        let events = common::read_events(self.run.path());
        assert_eq!(
            events
                .iter()
                .filter(|event| event["type"] == "tool_call_intent")
                .count(),
            1
        );
        assert!(!events
            .iter()
            .any(|event| event["type"] == "tool_call_result"));
        self.calls.lock().unwrap().push(call.clone());
        ensure!(
            !matches!(self.behavior, Behavior::Error),
            "scripted gateway error"
        );
        let mut paths = vec![];
        if matches!(
            self.behavior,
            Behavior::CompleteArtifact | Behavior::IncompleteArtifact
        ) {
            let written = artifacts::write_atomic(
                &self.run.artifacts_dir(),
                "page.json",
                br#"{"content":"exact page"}"#,
            )?;
            artifacts::write_meta(
                &written.path,
                &ArtifactMeta {
                    fetched_at: 0,
                    complete: matches!(self.behavior, Behavior::CompleteArtifact),
                },
            )?;
            paths.push(written.path);
        }
        Ok(ToolResult {
            content_for_model: json!({"ok":true}),
            artifacts: paths,
            provenance: Provenance::Live,
        })
    }
}

struct Harness {
    base: PathBuf,
    run: RunDir,
    prompts: Arc<Mutex<Vec<String>>>,
    calls: Arc<Mutex<Vec<ToolCall>>>,
}
impl Harness {
    fn new() -> Self {
        let base = common::tmp_dir("tool-result-completion");
        std::fs::write(base.join("work.md"), "Perform the requested tool call.").unwrap();
        Self {
            run: RunDir::under(&base),
            base,
            prompts: Arc::default(),
            calls: Arc::default(),
        }
    }
    fn executor(
        &self,
        plan: ExecutionPlan,
        registry: TaskRegistry,
        responses: &[&str],
        behavior: Behavior,
    ) -> Result<Executor<RecordingModel, Gateway>> {
        Executor::new(
            plan,
            registry,
            String::new(),
            SlotMap::new().with(
                "coder",
                RecordingModel {
                    model: MockModel::scripted(responses.iter().copied()),
                    prompts: self.prompts.clone(),
                },
            ),
            Gateway {
                run: self.run.clone(),
                calls: self.calls.clone(),
                behavior,
            },
            self.run.clone(),
        )
    }
    fn run(
        &self,
        registry: &str,
        responses: &[&str],
        behavior: Behavior,
    ) -> (RunStatus, Value, Vec<Value>) {
        let registry = TaskRegistry::from_yaml_str(registry, &self.base).unwrap();
        let mut executor = self
            .executor(
                ExecutionPlan::from_yaml_str(PLAN).unwrap(),
                registry,
                responses,
                behavior,
            )
            .unwrap();
        let status = executor.run_until_blocked().unwrap();
        let state = serde_json::to_value(executor.state()).unwrap();
        (status, state, common::read_events(self.run.path()))
    }
}
impl Drop for Harness {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.base).unwrap();
    }
}
fn event_count(events: &[Value], kind: &str) -> usize {
    events.iter().filter(|event| event["type"] == kind).count()
}

#[test]
fn one_successful_call_completes_without_second_generation() {
    let h = Harness::new();
    let (status, state, events) = h.run(REGISTRY, &[CALL], Behavior::Success);
    assert_eq!(status, RunStatus::Succeeded);
    assert_eq!(h.prompts.lock().unwrap().len(), 1);
    assert_eq!(h.calls.lock().unwrap().len(), 1);
    assert_eq!(state["counters"]["steps"], 1);
    assert_eq!(event_count(&events, "tool_call_intent"), 1);
    assert_eq!(event_count(&events, "tool_call_result"), 1);
    assert_eq!(state["stack"][0]["nodeOutputs"]["work"], json!({}));
}

#[test]
fn zero_or_multiple_calls_fail_before_any_intent_or_dispatch() {
    for response in [
        "Already done.".to_string(),
        String::new(),
        format!("{CALL}\n{CALL}"),
    ] {
        let h = Harness::new();
        let (status, _, events) = h.run(REGISTRY, &[&response], Behavior::Success);
        assert!(
            matches!(status, RunStatus::Failed { reason } if reason.contains("tool-call-parse"))
        );
        assert_eq!(h.prompts.lock().unwrap().len(), 1);
        assert!(h.calls.lock().unwrap().is_empty());
        assert_eq!(event_count(&events, "tool_call_intent"), 0);
        assert_eq!(event_count(&events, "tool_call_result"), 0);
    }
}

#[test]
fn denied_tools_and_invalid_arguments_still_fail_before_intent() {
    for response in [
        CALL.replace("write", "denied"),
        CALL.replace(r#""id":"7""#, r#""id":7"#),
    ] {
        let h = Harness::new();
        let (status, _, events) = h.run(REGISTRY, &[&response], Behavior::Success);
        assert!(
            matches!(status, RunStatus::Failed { reason } if reason.contains("tool-call-parse"))
        );
        assert!(h.calls.lock().unwrap().is_empty());
        assert_eq!(event_count(&events, "tool_call_intent"), 0);
    }
}

#[test]
fn gateway_error_cannot_complete_task() {
    let h = Harness::new();
    let (status, _, events) = h.run(REGISTRY, &[CALL], Behavior::Error);
    assert!(matches!(status, RunStatus::Failed { reason } if reason.contains("mcp-error")));
    assert_eq!(h.prompts.lock().unwrap().len(), 1);
    assert_eq!(h.calls.lock().unwrap().len(), 1);
    assert_eq!(event_count(&events, "tool_call_intent"), 1);
    assert_eq!(event_count(&events, "tool_call_result"), 0);
}

#[test]
fn artifact_outputs_are_verified_after_successful_call() {
    let registry = format!("{REGISTRY}\n    outputContract:\n      outputs:\n        page: {{ fromArtifact: page, format: json, type: object }}\n      requireArtifactComplete: [page]\n");
    for behavior in [
        Behavior::Success,
        Behavior::IncompleteArtifact,
        Behavior::CompleteArtifact,
    ] {
        let h = Harness::new();
        let (status, state, events) = h.run(&registry, &[CALL], behavior);
        assert_eq!(h.prompts.lock().unwrap().len(), 1);
        assert_eq!(h.calls.lock().unwrap().len(), 1);
        assert_eq!(event_count(&events, "tool_call_result"), 1);
        if matches!(behavior, Behavior::CompleteArtifact) {
            assert_eq!(status, RunStatus::Succeeded);
            assert_eq!(
                state["stack"][0]["nodeOutputs"]["work"]["page"],
                json!({"content":"exact page"})
            );
            assert_eq!(event_count(&events, "artifact_written"), 1);
        } else {
            assert!(
                matches!(status, RunStatus::Failed { reason } if reason.contains("verification-failed"))
            );
            assert!(state["stack"][0]["nodeOutputs"].get("work").is_none());
        }
    }
}

#[test]
fn invalid_completion_configuration_rejected_before_generation() {
    for registry in [
        REGISTRY.replace("tools: [t.write]", "tools: []"),
        format!("{REGISTRY}\n    loop: {{ maxSteps: 0 }}\n"),
        format!("{REGISTRY}\n    loop: {{ maxSteps: 2 }}\n"),
        format!("{REGISTRY}\n    outputContract: {{ allowNoop: true }}\n"),
        format!("{REGISTRY}\n    outputContract: {{ outputs: {{ report: {{ from: model, type: prose }} }} }}\n"),
    ] {
        let h = Harness::new();
        assert!(TaskRegistry::from_yaml_str(&registry, &h.base).unwrap_err().to_string().contains("tool-result"));
        // Constructor validation also covers registries assembled or mutated by callers.
        let registry: TaskRegistry = serde_yaml::from_str(&registry).unwrap();
        let error = h.executor(ExecutionPlan::from_yaml_str(PLAN).unwrap(), registry, &[CALL], Behavior::Success).err().unwrap();
        assert!(error.to_string().contains("tool-result"));
        assert!(h.prompts.lock().unwrap().is_empty());
        assert!(h.calls.lock().unwrap().is_empty());
    }
    let h = Harness::new();
    let registry = TaskRegistry::from_yaml_str(REGISTRY, &h.base).unwrap();
    let mut plan = ExecutionPlan::from_yaml_str(PLAN).unwrap();
    plan.nodes.get_mut("work").unwrap().outputs =
        Some(serde_yaml::from_str("{report: {from: model}}").unwrap());
    assert!(h
        .executor(plan, registry, &[CALL], Behavior::Success)
        .is_err());
    assert!(h.prompts.lock().unwrap().is_empty());
}

#[test]
fn omitted_or_explicit_model_prose_retains_result_then_generation_loop() {
    for completion in ["", "    completion: model-prose\n"] {
        let h = Harness::new();
        let registry = REGISTRY.replace("    completion: tool-result\n", completion) + "\n    loop: { maxSteps: 2 }\n    outputContract: { outputs: { report: { from: model, type: prose } } }\n";
        let parsed = TaskRegistry::from_yaml_str(&registry, &h.base).unwrap();
        assert_eq!(parsed.tasks["work"].completion, TaskCompletion::ModelProse);
        let (status, state, _) = h.run(&registry, &[CALL, "Finished."], Behavior::Success);
        assert_eq!(status, RunStatus::Succeeded);
        assert_eq!(h.calls.lock().unwrap().len(), 1);
        let prompts = h.prompts.lock().unwrap();
        assert_eq!(prompts.len(), 2);
        assert!(prompts[1].contains("<tool_result>"));
        assert_eq!(
            state["stack"][0]["nodeOutputs"]["work"]["report"],
            "Finished."
        );
    }
}
