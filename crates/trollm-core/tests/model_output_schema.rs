//! Model output contracts validate plain strings; no JSON interpretation or network.
mod common;

use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use trollm_core::{ExecutionPlan, Executor, RunDir, RunStatus, SlotMap, TaskRegistry};
use trollm_mcp::{Provenance, ToolCall, ToolDescriptor, ToolGateway, ToolResult};
use trollm_models::MockModel;

const CALL: &str = r#"<tool_call>{"server":"t","tool":"write","args":{}}</tool_call>"#;
const PLAN: &str = r#"
schemaVersion: ledger.execution-plan.v1
id: model-schema
version: 1
limits: { maxSteps: 4, maxReasonerCalls: 0, maxTokensTotal: 10000, timeoutS: 60, artifactMaxMb: 1 }
onLimit: abort
models: { coder: { ctx: 2048 } }
entryStep: query
nodes:
  query: { kind: task, taskRef: query, onVerified: write, onError: {then: failed} }
  write: { kind: task, taskRef: write, onVerified: done }
  done: { kind: end, outcome: succeeded }
  failed: { kind: end, outcome: failed }
"#;

struct Gateway {
    calls: Arc<Mutex<Vec<ToolCall>>>,
}
impl ToolGateway for Gateway {
    fn list_tools(&self) -> anyhow::Result<Vec<ToolDescriptor>> {
        Ok(vec![])
    }
    fn tool_class(&self, _: &str, _: &str) -> trollm_mcp::policy::ToolClass {
        trollm_mcp::policy::ToolClass::Mutation
    }
    fn validate_call(&self, call: &ToolCall) -> anyhow::Result<()> {
        anyhow::ensure!(call.args["value"].is_string());
        Ok(())
    }
    fn call(&mut self, call: &ToolCall) -> anyhow::Result<ToolResult> {
        self.calls.lock().unwrap().push(call.clone());
        Ok(ToolResult {
            content_for_model: json!({"ok":true}),
            artifacts: vec![],
            provenance: Provenance::Local,
        })
    }
}

fn registry(schema: Option<Value>, extra: bool) -> String {
    let mut output = json!({"from":"model","type":"string"});
    if let Some(schema) = schema {
        output["schema"] = schema;
    }
    let mut outputs = json!({"query":output});
    if extra {
        outputs["second"] = json!({"from":"model","type":"string","schema":{"const":"different"}});
    }
    serde_yaml::to_string(&json!({"schemaVersion":"ledger.task-registry.v1","tasks":{
        "query":{"model":"coder","prompt":"query.md","tools":[],"outputContract":{"outputs":outputs}},
        "write":{"model":"coder","prompt":"write.md","tools":["t.write"],"completion":"tool-result",
            "toolArguments":{"t.write":{"modelFields":[],"fromVars":{"value":"query.outputs.query"}}},
            "mutations":{"idempotency":"intent-log","onUncertain":"fail-run"}}
    }})).unwrap()
}

struct ResultEvidence {
    status: RunStatus,
    vars: Value,
    events: Vec<Value>,
    diagnostics: Vec<Value>,
    calls: Vec<ToolCall>,
}
fn run(
    response: &str,
    schema: Option<Value>,
    same_override: bool,
    extra: bool,
    artifact_mb: u64,
) -> ResultEvidence {
    let base = common::tmp_dir("model-output-schema");
    std::fs::write(base.join("query.md"), "Generate one plain query.").unwrap();
    std::fs::write(base.join("write.md"), "Invoke the bound tool.").unwrap();
    let mut plan: Value = serde_yaml::from_str(PLAN).unwrap();
    plan["limits"]["artifactMaxMb"] = json!(artifact_mb);
    if same_override {
        plan["nodes"]["query"]["outputs"] = json!({"query":{"from":"model"}});
    }
    let run = RunDir::under(&base);
    let calls = Arc::new(Mutex::new(vec![]));
    let mut executor = Executor::new(
        ExecutionPlan::from_yaml_str(&serde_yaml::to_string(&plan).unwrap()).unwrap(),
        TaskRegistry::from_yaml_str(&registry(schema, extra), &base).unwrap(),
        String::new(),
        SlotMap::new().with("coder", MockModel::scripted([response, CALL])),
        Gateway {
            calls: calls.clone(),
        },
        run.clone(),
    )
    .unwrap();
    let status = executor.run_until_blocked().unwrap();
    let diagnostics = std::fs::read_dir(run.artifacts_dir())
        .unwrap()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("model-failure-")
                && !path.to_string_lossy().ends_with(".meta.json")
        })
        .map(|path| {
            assert!(trollm_core::artifacts::is_complete(&path));
            serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
        })
        .collect();
    let result = ResultEvidence {
        status,
        vars: executor.state().stack[0].vars.clone(),
        events: common::read_events(run.path()),
        diagnostics,
        calls: calls.lock().unwrap().clone(),
    };
    std::fs::remove_dir_all(base).unwrap();
    result
}

fn nonempty() -> Value {
    json!({"type":"string","minLength":1})
}

#[test]
fn empty_and_whitespace_fail_before_binding_mutation_and_keep_raw_response() {
    for response in ["", " \t\r\n"] {
        for same_override in [false, true] {
            let result = run(response, Some(nonempty()), same_override, false, 1);
            assert!(matches!(result.status, RunStatus::Failed { .. }));
            assert!(result.calls.is_empty());
            assert!(result.vars.get("query").is_none());
            assert!(!result
                .events
                .iter()
                .any(|event| event["type"] == "tool_call_intent"));
            let error = result
                .events
                .iter()
                .find(|event| event["type"] == "error")
                .unwrap();
            assert_eq!(error["class"], "verification-failed");
            assert_eq!(
                error["detail"],
                "output 'query' model schema validation failed"
            );
            assert_eq!(result.diagnostics.len(), 1);
            assert_eq!(result.diagnostics[0]["response"]["text"]["text"], response);
            assert_eq!(result.diagnostics[0]["response"]["finishReason"], "stop");
            assert_eq!(
                result.diagnostics[0]["request"]["prompt"]["text"],
                "Generate one plain query."
            );
        }
    }
}

#[test]
fn valid_trimmed_strings_and_same_source_overrides_publish_exactly_without_json_parsing() {
    for response in ["  yellow rubber duck\n", "  {\"query\":\"literal text\"}  "] {
        for same_override in [false, true] {
            let result = run(response, Some(nonempty()), same_override, false, 1);
            assert_eq!(result.status, RunStatus::Succeeded);
            assert_eq!(result.calls.len(), 1);
            assert_eq!(result.vars["query"]["outputs"]["query"], response.trim());
            assert_eq!(result.calls[0].args["value"], response.trim());
            assert!(result.diagnostics.is_empty());
        }
    }
}

#[test]
fn every_model_contract_is_checked_even_when_another_output_has_a_node_override() {
    let result = run("valid query", Some(nonempty()), true, true, 1);
    assert!(matches!(result.status, RunStatus::Failed { .. }));
    assert!(result.calls.is_empty());
    assert!(result.vars.get("query").is_none());
    assert!(result
        .events
        .iter()
        .any(|event| event["detail"] == "output 'second' model schema validation failed"));
    assert_eq!(
        result.diagnostics[0]["response"]["text"]["text"],
        "valid query"
    );
}

#[test]
fn invalid_schemas_and_incompatible_source_overrides_reject_before_execution() {
    let base = common::tmp_dir("model-schema-source-validation");
    std::fs::write(base.join("query.md"), "Generate query.").unwrap();
    std::fs::write(base.join("write.md"), "Write query.").unwrap();
    for invalid in [
        Value::Null,
        json!(false),
        json!({"type":"nonsense"}),
        json!({"pattern":"["}),
        json!({"$ref":"https://invalid.example/private"}),
        json!({"description":"x".repeat(65536)}),
    ] {
        assert!(TaskRegistry::from_yaml_str(&registry(Some(invalid), false), ".").is_err());
    }
    let mut plan: Value = serde_yaml::from_str(PLAN).unwrap();
    plan["nodes"]["query"]["outputs"] =
        json!({"query":{"fromArtifact":"unrelated","format":"json"}});
    let plan = ExecutionPlan::from_yaml_str(&serde_yaml::to_string(&plan).unwrap()).unwrap();
    let guarded = TaskRegistry::from_yaml_str(&registry(Some(nonempty()), false), &base).unwrap();
    let error = trollm_core::validate::validate_with_registry(&plan, &guarded)
        .unwrap_err()
        .to_string();
    assert!(error.contains("schema-bearing model output"));
    let legacy = TaskRegistry::from_yaml_str(&registry(None, false), &base).unwrap();
    assert!(trollm_core::validate::validate_with_registry(&plan, &legacy).is_ok());
    std::fs::remove_dir_all(base).unwrap();
}

#[test]
fn legacy_empty_outputs_and_schema_validation_need_no_artifact_allocation() {
    assert_eq!(
        run(" \n", None, false, false, 0).status,
        RunStatus::Succeeded
    );
    assert_eq!(
        run("query", Some(nonempty()), false, false, 0).status,
        RunStatus::Succeeded
    );
    assert_eq!(
        run(&"x".repeat(300_000), Some(nonempty()), false, false, 0).status,
        RunStatus::Succeeded
    );
    let failure = run(" \n", Some(nonempty()), false, false, 0);
    assert!(matches!(failure.status, RunStatus::Failed { .. }));
    assert!(failure.calls.is_empty());
    assert!(failure.diagnostics.is_empty());
}

#[test]
fn schema_rejection_keeps_instruction_header_text_private_and_reports_no_instance_values() {
    let raw = "  [Task registry instructions] PRIVATE response  \n";
    let result = run(
        raw,
        Some(json!({"type":"string","enum":["accepted query"]})),
        false,
        false,
        1,
    );
    assert!(matches!(result.status, RunStatus::Failed { .. }));
    assert!(result.calls.is_empty());
    assert_eq!(result.diagnostics[0]["response"]["text"]["text"], raw);
    assert!(result
        .events
        .iter()
        .filter(|event| event["type"] == "error")
        .all(|event| !event.to_string().contains("PRIVATE response")));
}
