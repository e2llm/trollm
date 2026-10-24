//! Completion requirements must reach the generation boundary, not just dispatch.
mod common;

use anyhow::Result;
use serde_json::Value;
use std::sync::{Arc, Mutex};
use trollm_core::{ExecutionPlan, Executor, RunDir, RunStatus, SlotMap, TaskRegistry};
use trollm_mcp::{ToolCall, ToolDescriptor, ToolGateway, ToolResult};
use trollm_models::fence::{tool_call_grammar_with_argument_schemas, ToolCallGrammarMode};
use trollm_models::{GenerateRequest, GenerateResponse, MockModel, ModelSlot};

const READ: &str = r#"<tool_call>{"server":"t","tool":"read","args":{}}</tool_call>"#;
const CHOOSE: &str =
    r#"<tool_call>{"server":"flow","tool":"choose","args":{"route":"finish"}}</tool_call>"#;

struct RecordingModel {
    model: MockModel,
    requests: Arc<Mutex<Vec<GenerateRequest>>>,
}

impl ModelSlot for RecordingModel {
    fn generate(&mut self, request: GenerateRequest) -> Result<GenerateResponse> {
        self.requests.lock().unwrap().push(request.clone());
        self.model.generate(request)
    }
}

struct DiscoveryGateway {
    inner: common::ScriptGateway,
    empty_catalog: bool,
    dispatched: Arc<Mutex<Vec<ToolCall>>>,
}

impl ToolGateway for DiscoveryGateway {
    fn list_tools(&self) -> Result<Vec<ToolDescriptor>> {
        panic!("task guidance must never discover remote catalogs");
    }

    fn available_tools(&self, grants: &[String]) -> Vec<ToolDescriptor> {
        if self.empty_catalog {
            return vec![];
        }
        self.inner.available_tools(grants)
    }

    fn tool_class(&self, server: &str, tool: &str) -> trollm_mcp::policy::ToolClass {
        self.inner.tool_class(server, tool)
    }

    fn validate_call(&self, call: &ToolCall) -> Result<()> {
        self.inner.validate_call(call)
    }

    fn call(&mut self, call: &ToolCall) -> Result<ToolResult> {
        self.dispatched.lock().unwrap().push(call.clone());
        self.inner.call(call)
    }
}

fn run(
    completion: &str,
    slot: &str,
    tools: &[&str],
    responses: &[&str],
) -> (RunStatus, Vec<GenerateRequest>, usize, Vec<Value>) {
    run_with_catalog(completion, slot, tools, responses, false)
}

fn run_with_catalog(
    completion: &str,
    slot: &str,
    tools: &[&str],
    responses: &[&str],
    empty_catalog: bool,
) -> (RunStatus, Vec<GenerateRequest>, usize, Vec<Value>) {
    let base = common::tmp_dir("completion-grammar");
    std::fs::write(base.join("work.md"), "Perform this task.").unwrap();
    let transition = if completion == "flow-route" {
        "routes: { finish: done }"
    } else {
        "onVerified: done"
    };
    let plan = format!(
        "schemaVersion: ledger.execution-plan.v1\nid: grammar\nversion: 1\nlimits: {{ maxSteps: 8, maxReasonerCalls: 2, maxTokensTotal: 10000, timeoutS: 60, artifactMaxMb: 1 }}\nonLimit: abort\nmodels: {{ {slot}: {{ ctx: 2048 }} }}\nentryStep: work\nnodes:\n  work: {{ kind: task, taskRef: work, {transition} }}\n  done: {{ kind: end, outcome: succeeded }}\n"
    );
    let max_steps = if completion == "tool-result" { 1 } else { 3 };
    let registry = format!(
        "schemaVersion: ledger.task-registry.v1\ntasks:\n  work:\n    model: {slot}\n    prompt: work.md\n    completion: {completion}\n    tools: [{}]\n    loop: {{ maxSteps: {max_steps} }}\n",
        tools.join(", ")
    );
    let dir = RunDir::under(&base);
    let requests = Arc::new(Mutex::new(Vec::new()));
    let dispatched = Arc::new(Mutex::new(Vec::new()));
    let mut executor = Executor::new(
        ExecutionPlan::from_yaml_str(&plan).unwrap(),
        TaskRegistry::from_yaml_str(&registry, &base).unwrap(),
        String::new(),
        SlotMap::new().with(
            slot,
            RecordingModel {
                model: MockModel::scripted(responses.iter().copied()),
                requests: requests.clone(),
            },
        ),
        DiscoveryGateway {
            inner: common::ScriptGateway::new(dir.artifacts_dir()),
            empty_catalog,
            dispatched: dispatched.clone(),
        },
        dir.clone(),
    )
    .unwrap();
    let status = executor.run_until_blocked().unwrap();
    let events = common::read_events(dir.path());
    // Local flow.choose emits tool_call_result too; count real gateway calls.
    let dispatched = dispatched.lock().unwrap();
    assert!(dispatched.iter().all(|call| call.server != "flow"));
    let calls = dispatched.len();
    let requests = requests.lock().unwrap().clone();
    std::fs::remove_dir_all(base).unwrap();
    (status, requests, calls, events)
}

#[test]
fn completion_selects_generation_contract_and_preserves_ordinary_batches() {
    let batch = format!("{READ}{READ}");
    for (completion, tools, responses, mode, call_count) in [
        (
            "tool-result",
            vec!["t.read"],
            vec![READ],
            ToolCallGrammarMode::SingleToolCall,
            1,
        ),
        (
            "flow-route",
            vec!["t.read", "flow.choose"],
            vec![batch.as_str(), CHOOSE],
            ToolCallGrammarMode::ToolCallsOnly,
            2,
        ),
        (
            "model-prose",
            vec!["t.read"],
            vec![batch.as_str(), "Complete."],
            ToolCallGrammarMode::ProseOrToolCalls,
            2,
        ),
    ] {
        let (status, requests, calls, _) = run(completion, "coder", &tools, &responses);
        assert_eq!(status, RunStatus::Succeeded);
        assert_eq!(calls, call_count);
        let grants = tools
            .iter()
            .map(|grant| {
                let (server, tool) = grant.split_once('.').unwrap();
                (server.to_string(), tool.to_string())
            })
            .collect::<Vec<_>>();
        let mut schemas = std::collections::BTreeMap::new();
        if tools.contains(&"flow.choose") {
            schemas.insert(
                ("flow".into(), "choose".into()),
                serde_json::json!({
                    "type":"object", "properties":{"route":{"type":"string", "enum":["finish"]}},
                    "required":["route"], "additionalProperties":false
                }),
            );
        }
        let expected = tool_call_grammar_with_argument_schemas(&grants, &schemas, mode);
        // Native grammar tests own language semantics; here check the actual
        // requests on every iteration use the selected public grammar contract.
        assert!(requests
            .iter()
            .all(|request| request.grammar_gbnf.as_ref() == Some(&expected)));
    }
}

#[test]
fn tool_free_reasoner_keeps_plain_prompt_and_no_grammar() {
    let (status, requests, calls, _) = run("model-prose", "reasoner", &[], &["Advice."]);
    assert_eq!(status, RunStatus::Succeeded);
    assert_eq!(calls, 0);
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].prompt, "Perform this task.");
    assert!(requests[0].grammar_gbnf.is_none());
}

#[test]
fn tool_result_rejection_reports_only_parsed_call_count() {
    let batch = format!("{READ}{READ}");
    for (response, count) in [("PRIVATE_MODEL_PROSE", 0), (batch.as_str(), 2)] {
        let (status, _, calls, events) = run("tool-result", "coder", &["t.read"], &[response]);
        let RunStatus::Failed { reason } = status else {
            panic!("invalid tool-result generation must fail");
        };
        // Default onError stores the error class as the terminal reason. The
        // structured error event retains the safe diagnostic detail.
        assert!(
            reason.contains("tool-call-parse"),
            "terminal reason: {reason}"
        );
        let error = events
            .iter()
            .find(|event| event["type"] == "error")
            .expect("failed task must emit a structured error event");
        assert_eq!(
            error["class"], "tool-call-parse",
            "terminal reason: {reason}"
        );
        assert_eq!(
            error["detail"],
            format!("tool-result completion requires exactly one tool call; parsed {count} calls"),
            "terminal reason: {reason}"
        );
        assert!(!error["detail"]
            .as_str()
            .unwrap()
            .contains("PRIVATE_MODEL_PROSE"));
        assert_eq!(calls, 0);
        assert!(!events
            .iter()
            .any(|event| event["type"] == "tool_call_intent"));
    }
}

#[test]
fn local_control_never_discovers_unrelated_remote_catalogs() {
    // Both sole and mixed grants must use only network-free guidance.
    for tools in [vec!["flow.choose"], vec!["t.read", "flow.choose"]] {
        let (status, requests, calls, _) =
            run_with_catalog("flow-route", "coder", &tools, &[CHOOSE], true);
        assert_eq!(status, RunStatus::Succeeded);
        assert_eq!(requests.len(), 1);
        assert_eq!(calls, 0);
        let grammar = requests[0].grammar_gbnf.as_ref().unwrap();
        assert!(grammar.contains(if tools.len() == 1 {
            "root ::= ws fence ws"
        } else {
            "root ::= ws fence (ws fence)* ws"
        }));
        assert!(grammar.contains("enum-a"));
        assert!(grammar.contains("\\\"finish\\\""));
    }
}

#[test]
fn local_calculator_never_discovers_unrelated_remote_catalogs() {
    // This fixture checks dispatch isolation; production calculator semantics
    // and its locally available descriptor are covered in MCP tests.
    let response = r#"<tool_call>{"server":"local","tool":"calculate","args":{}}</tool_call>"#;
    let (status, requests, calls, _) = run_with_catalog(
        "tool-result",
        "coder",
        &["local.calculate", "unavailable.remote"],
        &[response],
        true,
    );
    assert_eq!(status, RunStatus::Succeeded);
    assert_eq!(requests.len(), 1);
    assert_eq!(calls, 1);
}

#[test]
fn setup_required_open_capture_arguments_reach_decoder_after_bindings() {
    struct CaptureGateway {
        cached: bool,
    }
    impl ToolGateway for CaptureGateway {
        fn list_tools(&self) -> Result<Vec<ToolDescriptor>> {
            panic!("task guidance must never discover remote catalogs");
        }
        fn available_tools(&self, _: &[String]) -> Vec<ToolDescriptor> {
            if !self.cached {
                return vec![];
            }
            vec![ToolDescriptor {
                server: "browser".into(),
                name: "capture".into(),
                input_schema: serde_json::json!({
                    "type":"object", "properties":{"tabId":{"type":"number"}, "browser":{"type":"string"}, "forceFresh":{"type":"boolean"}}
                }),
            }]
        }
        fn argument_constraints(&self, _: &str, _: &str) -> Option<Value> {
            Some(serde_json::json!({"type":"object", "required":["tabId"]}))
        }
        fn tool_class(&self, _: &str, _: &str) -> trollm_mcp::policy::ToolClass {
            trollm_mcp::policy::ToolClass::Read
        }
        fn validate_call(&self, call: &ToolCall) -> Result<()> {
            anyhow::ensure!(call.args["tabId"].is_number(), "required tabId");
            anyhow::ensure!(call.args["browser"] == "selected", "injected browser");
            anyhow::ensure!(call.args["forceFresh"] == true, "injected forceFresh");
            Ok(())
        }
        fn call(&mut self, call: &ToolCall) -> Result<ToolResult> {
            self.validate_call(call)?;
            Ok(ToolResult {
                content_for_model: serde_json::json!({"ok":true}),
                artifacts: vec![],
                provenance: trollm_mcp::Provenance::Local,
            })
        }
    }
    for cached in [true, false] {
        let base = common::tmp_dir("required-open-capture");
        std::fs::write(base.join("work.md"), "Observe.").unwrap();
        let plan = ExecutionPlan::from_yaml_str("schemaVersion: ledger.execution-plan.v1\nid: capture\nversion: 1\nlimits: { maxSteps: 4, maxReasonerCalls: 0, maxTokensTotal: 10000, timeoutS: 60, artifactMaxMb: 1 }\nonLimit: abort\nmodels: { coder: { ctx: 2048 } }\nentryStep: work\nnodes:\n  work: { kind: task, taskRef: work, onVerified: done }\n  done: { kind: end, outcome: succeeded }\n").unwrap();
        let registry = TaskRegistry::from_yaml_str("schemaVersion: ledger.task-registry.v1\ntasks:\n  work:\n    model: coder\n    prompt: work.md\n    completion: tool-result\n    tools: [browser.capture]\n    toolArguments:\n      browser.capture:\n        fixed: { browser: selected, forceFresh: true }\n", &base).unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let dir = RunDir::under(&base);
        let mut executor = Executor::new(plan, registry, String::new(), SlotMap::new().with("coder", RecordingModel {
        model: MockModel::scripted([r#"<tool_call>{"server":"browser","tool":"capture","args":{"tabId":2,"extra":true}}</tool_call>"#]), requests: requests.clone()
    }), CaptureGateway { cached }, dir).unwrap();
        assert_eq!(executor.run_until_blocked().unwrap(), RunStatus::Succeeded);
        let requests = requests.lock().unwrap();
        let grammar = requests[0].grammar_gbnf.as_ref().unwrap();
        let required_rule = grammar
            .lines()
            .find(|line| line.starts_with("first-a0-0 ::="))
            .unwrap();
        assert!(required_rule.contains("tabId"));
        assert!(required_rule.contains(if cached {
            "ws number tail-a0-1"
        } else {
            "ws value tail-a0-1"
        }));
        assert!(
            !required_rule.contains(" | "),
            "empty arguments cannot skip tabId"
        );
        assert!(
            grammar.contains("tail-a0-1 ::= (\",\" ws member)*"),
            "additional arguments remain legal"
        );
        assert!(requests[0]
            .prompt
            .contains("Required model arguments for browser.capture"));
        drop(executor);
        std::fs::remove_dir_all(base).unwrap();
    }
}
