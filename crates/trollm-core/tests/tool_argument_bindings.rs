//! Typed frame-variable argument binding; scripted models/gateway only.
mod common;

use anyhow::{ensure, Result};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use trollm_core::artifacts::{self, ArtifactMeta};
use trollm_core::{ExecutionPlan, Executor, RunDir, RunStatus, SlotMap, TaskRegistry};
use trollm_mcp::{Provenance, ToolCall, ToolDescriptor, ToolGateway, ToolResult};
use trollm_models::{GenerateRequest, GenerateResponse, MockModel, ModelSlot};

const PLAN: &str = r#"
schemaVersion: ledger.execution-plan.v1
id: argument-bindings
version: 1
limits: { maxSteps: 2, maxReasonerCalls: 0, maxTokensTotal: 10000, timeoutS: 60, artifactMaxMb: 1 }
onLimit: abort
models: { coder: { ctx: 2048 } }
entryStep: opened
nodes:
  opened: { kind: task, taskRef: opened, onVerified: read }
  read: { kind: task, taskRef: read, onVerified: done }
  done: { kind: end, outcome: succeeded }
"#;
const REGISTRY: &str = r#"
schemaVersion: ledger.task-registry.v1
tasks:
  opened:
    model: coder
    prompt: work.md
    completion: tool-result
    tools: [t.open]
    outputContract:
      outputs:
        opened: { fromArtifact: opened, format: json, type: object }
  read:
    model: coder
    prompt: work.md
    completion: tool-result
    tools: [t.read]
    toolArguments:
      t.read:
        fromVars: { tabId: opened.outputs.opened.tabId }
    mutations: { idempotency: intent-log, onUncertain: fail-run }
"#;
const OPEN: &str = r#"<tool_call>{"server":"t","tool":"open","args":{}}</tool_call>"#;

struct RecordingModel {
    inner: MockModel,
    calls: Arc<Mutex<usize>>,
}
impl ModelSlot for RecordingModel {
    fn generate(&mut self, request: GenerateRequest) -> Result<GenerateResponse> {
        *self.calls.lock().unwrap() += 1;
        self.inner.generate(request)
    }
}
struct Gateway {
    run: RunDir,
    calls: Arc<Mutex<Vec<ToolCall>>>,
}
impl ToolGateway for Gateway {
    fn tool_class(&self, _: &str, _: &str) -> trollm_mcp::policy::ToolClass {
        trollm_mcp::policy::ToolClass::Read
    }
    fn list_tools(&self) -> Result<Vec<ToolDescriptor>> {
        Ok(vec![])
    }
    fn validate_call(&self, call: &ToolCall) -> Result<()> {
        if call.tool == "read" {
            // Injection must precede schema validation; the original integer
            // must not become a JSON string, and the other arguments survive.
            ensure!(
                call.args["tabId"].as_u64() == Some(178),
                "wrong bound integer"
            );
            ensure!(call.args["browser"] == "fixture", "other args changed");
            assert!(!common::read_events(self.run.path())
                .iter()
                .any(|event| event["type"] == "tool_call_intent"));
        }
        Ok(())
    }
    fn call(&mut self, call: &ToolCall) -> Result<ToolResult> {
        self.calls.lock().unwrap().push(call.clone());
        let mut paths = vec![];
        if call.tool == "open" {
            let written = artifacts::write_atomic(
                &self.run.artifacts_dir(),
                "opened.json",
                br#"{"tabId":178}"#,
            )?;
            artifacts::write_meta(
                &written.path,
                &ArtifactMeta {
                    fetched_at: 0,
                    complete: true,
                },
            )?;
            paths.push(written.path);
        } else {
            let intents: Vec<_> = common::read_events(self.run.path())
                .into_iter()
                .filter(|event| event["type"] == "tool_call_intent")
                .collect();
            assert_eq!(intents.len(), 1);
            assert_eq!(intents[0]["args"], call.args);
            assert_eq!(
                intents[0]["argsSha256"],
                trollm_core::plan::hex_sha256(call.args.to_string().as_bytes())
            );
        }
        Ok(ToolResult {
            content_for_model: json!({"ok":true}),
            artifacts: paths,
            provenance: Provenance::Live,
        })
    }
}
struct Fixture {
    base: PathBuf,
    run: RunDir,
    model_calls: Arc<Mutex<usize>>,
    gateway_calls: Arc<Mutex<Vec<ToolCall>>>,
}
impl Fixture {
    fn new() -> Self {
        let base = common::tmp_dir("tool-argument-bindings");
        std::fs::write(base.join("work.md"), "Perform the granted call.").unwrap();
        Self {
            run: RunDir::under(&base),
            base,
            model_calls: Arc::default(),
            gateway_calls: Arc::default(),
        }
    }
    fn run(&self, registry: &str, args: Value) -> (RunStatus, Vec<Value>) {
        let response = format!(
            "<tool_call>{}</tool_call>",
            json!({"server":"t","tool":"read","args":args})
        );
        let mut exec = Executor::new(
            ExecutionPlan::from_yaml_str(PLAN).unwrap(),
            TaskRegistry::from_yaml_str(registry, &self.base).unwrap(),
            String::new(),
            SlotMap::new().with(
                "coder",
                RecordingModel {
                    inner: MockModel::scripted([OPEN, &response]),
                    calls: self.model_calls.clone(),
                },
            ),
            Gateway {
                run: self.run.clone(),
                calls: self.gateway_calls.clone(),
            },
            self.run.clone(),
        )
        .unwrap();
        let status = exec.run_until_blocked().unwrap();
        (status, common::read_events(self.run.path()))
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.base).unwrap();
    }
}

#[test]
fn absent_or_matching_owned_argument_uses_original_integer_before_validation_and_journal() {
    for args in [
        json!({"browser":"fixture"}),
        json!({"browser":"fixture","tabId":178}),
    ] {
        let fixture = Fixture::new();
        let (status, _) = fixture.run(REGISTRY, args);
        assert_eq!(status, RunStatus::Succeeded);
        assert_eq!(*fixture.model_calls.lock().unwrap(), 2);
        let calls = fixture.gateway_calls.lock().unwrap();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[1].args, json!({"browser":"fixture","tabId":178}));
    }
}

#[test]
fn conflicting_owned_argument_never_dispatches_or_journals() {
    for id in [json!(137), json!("178"), Value::Null] {
        let fixture = Fixture::new();
        let (status, events) = fixture.run(REGISTRY, json!({"browser":"fixture","tabId":id}));
        assert!(
            matches!(status, RunStatus::Failed { reason } if reason.contains("tool-call-parse"))
        );
        assert_eq!(*fixture.model_calls.lock().unwrap(), 2);
        assert_eq!(fixture.gateway_calls.lock().unwrap().len(), 1);
        assert!(!events
            .iter()
            .any(|event| event["type"] == "tool_call_intent"));
        let failure = events
            .iter()
            .find(|event| event["type"] == "error")
            .unwrap();
        assert!(failure["detail"]
            .as_str()
            .unwrap()
            .contains("conflicts with fromVars"));
        assert!(!failure["detail"].as_str().unwrap().contains("137"));
    }
}

#[test]
fn bound_arguments_do_not_coerce_nonobject_model_args() {
    for args in [Value::Null, json!([]), json!("not an object")] {
        let fixture = Fixture::new();
        let (status, events) = fixture.run(REGISTRY, args);
        assert!(
            matches!(status, RunStatus::Failed { reason } if reason.contains("tool-call-parse"))
        );
        assert_eq!(*fixture.model_calls.lock().unwrap(), 2);
        assert_eq!(fixture.gateway_calls.lock().unwrap().len(), 1);
        assert!(!events
            .iter()
            .any(|event| event["type"] == "tool_call_intent"));
    }
}

#[test]
fn missing_binding_fails_before_that_tasks_generation() {
    let fixture = Fixture::new();
    // The first task cannot read its own not-yet-produced artifact.
    let registry = REGISTRY.replace("    tools: [t.open]", "    tools: [t.open]\n    toolArguments:\n      t.open:\n        fromVars: { tabId: opened.outputs.opened.tabId }");
    let (status, events) = fixture.run(&registry, json!({"browser":"fixture"}));
    assert!(
        matches!(status, RunStatus::Failed { reason } if reason.contains("verification-failed"))
    );
    assert_eq!(*fixture.model_calls.lock().unwrap(), 0);
    assert!(fixture.gateway_calls.lock().unwrap().is_empty());
    assert!(!events.iter().any(|event| matches!(
        event["type"].as_str(),
        Some("model_call" | "tool_call_intent" | "tool_call_result")
    )));
}

#[test]
fn missing_later_binding_does_not_generate_or_dispatch_read() {
    let fixture = Fixture::new();
    let registry = REGISTRY.replace(
        "opened.outputs.opened.tabId",
        "opened.outputs.opened.missing",
    );
    let (status, events) = fixture.run(&registry, json!({"browser":"fixture"}));
    assert!(
        matches!(status, RunStatus::Failed { reason } if reason.contains("verification-failed"))
    );
    assert_eq!(*fixture.model_calls.lock().unwrap(), 1);
    assert_eq!(fixture.gateway_calls.lock().unwrap().len(), 1);
    assert!(!events
        .iter()
        .any(|event| event["type"] == "tool_call_intent"));
}

#[test]
fn malformed_or_ungranted_binding_rules_are_rejected_at_load() {
    let fixture = Fixture::new();
    let binding =
        "toolArguments:\n      t.read:\n        fromVars: { tabId: opened.outputs.opened.tabId }";
    for invalid in [
        "toolArguments: {}",
        "toolArguments: { t.read: { fromVars: {} } }",
        "toolArguments: { t.denied: { fromVars: { tabId: item.id } } }",
        "toolArguments: { t.read: { fromVars: { '': item.id } } }",
        "toolArguments: { t.read: { fromVars: { tabId: '' } } }",
        "toolArguments: { t.read: { fromVars: { tabId: item..id } } }",
        "toolArguments: { t.read: { fromVars: { tabId: 'item.id | json' } } }",
        "toolArguments: { t.read: { fromVars: { tabId: item.id }, unknown: {} } }",
        "toolArguments: { t.read: { fromInput: { tabId: id } } }",
        "toolArguments: { t.read: { fromVars: { tabId: { from: item.id } } } }",
    ] {
        let registry = REGISTRY.replace(binding, invalid);
        assert!(
            TaskRegistry::from_yaml_str(&registry, &fixture.base).is_err(),
            "{invalid}"
        );
    }
    let registry = REGISTRY.replace("completion: tool-result", "completion: model-prose");
    assert!(TaskRegistry::from_yaml_str(&registry, &fixture.base)
        .unwrap_err()
        .to_string()
        .contains("requires tool-result"));
}

#[test]
fn combined_validation_rejects_programmatically_invalid_bindings() {
    let fixture = Fixture::new();
    let mut registry = TaskRegistry::from_yaml_str(REGISTRY, &fixture.base).unwrap();
    registry
        .tasks
        .get_mut("read")
        .unwrap()
        .tool_arguments
        .as_mut()
        .unwrap()
        .get_mut("t.read")
        .unwrap()
        .from_vars
        .clear();
    let error = trollm_core::validate::validate_with_registry(
        &ExecutionPlan::from_yaml_str(PLAN).unwrap(),
        &registry,
    )
    .unwrap_err();
    assert!(error.to_string().contains("fromVars must not be empty"));
    assert_eq!(*fixture.model_calls.lock().unwrap(), 0);
}
