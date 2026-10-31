//! Generic artifact contracts with scripted tools/models; no network or weights.
mod common;

use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use trollm_core::artifacts::{self, ArtifactMeta};
use trollm_core::{ExecutionPlan, Executor, RunDir, RunStatus, SlotMap, TaskRegistry};
use trollm_mcp::{Provenance, ToolCall, ToolDescriptor, ToolGateway, ToolResult};
use trollm_models::MockModel;

const CALL: &str = r#"<tool_call>{"server":"t","tool":"read","args":{}}</tool_call>"#;
const PLAN: &str = r#"
schemaVersion: ledger.execution-plan.v1
id: artifact-schema
version: 1
limits: { maxSteps: 4, maxReasonerCalls: 0, maxTokensTotal: 10000, timeoutS: 60, artifactMaxMb: 2 }
onLimit: abort
models: { coder: { ctx: 2048 } }
entryStep: read
nodes:
  read: { kind: task, taskRef: read, onVerified: downstream, onError: {then: failed} }
  downstream: { kind: task, taskRef: read, onVerified: done }
  done: { kind: end, outcome: succeeded }
  failed: { kind: end, outcome: failed }
"#;

struct Gateway {
    run: RunDir,
    payload: Value,
    calls: Arc<Mutex<usize>>,
    jsonl: bool,
}
impl ToolGateway for Gateway {
    fn list_tools(&self) -> anyhow::Result<Vec<ToolDescriptor>> {
        Ok(vec![])
    }
    fn tool_class(&self, _: &str, _: &str) -> trollm_mcp::policy::ToolClass {
        trollm_mcp::policy::ToolClass::Read
    }
    fn validate_call(&self, _: &ToolCall) -> anyhow::Result<()> {
        Ok(())
    }
    fn call(&mut self, _: &ToolCall) -> anyhow::Result<ToolResult> {
        *self.calls.lock().unwrap() += 1;
        let bytes = if self.jsonl {
            self.payload
                .as_array()
                .unwrap()
                .iter()
                .map(|value| format!("{value}\n"))
                .collect::<String>()
                .into_bytes()
        } else {
            serde_json::to_vec(&self.payload)?
        };
        let path =
            artifacts::write_atomic(&self.run.artifacts_dir(), "original.json", &bytes)?.path;
        artifacts::write_meta(
            &path,
            &ArtifactMeta {
                fetched_at: 0,
                complete: true,
            },
        )?;
        Ok(ToolResult {
            content_for_model: json!({"ok":true}),
            artifacts: vec![path],
            provenance: Provenance::Local,
        })
    }
}

fn schema() -> Value {
    json!({"type":"object","required":["tag","attributes"],"properties":{
        "tag":{"enum":["a"]}, "attributes":{"type":"object","required":["href"],
        "properties":{"href":{"type":"string","pattern":"^#[A-Za-z0-9_-]+$"}}}}})
}

fn registry(schema: Option<Value>, view: bool, jsonl: bool) -> String {
    let mut output = json!({"fromArtifact":"original","format":if jsonl {"jsonl"} else {"json"},"type":if jsonl {"list"} else {"object"}});
    if let Some(schema) = schema {
        output["schema"] = schema;
    }
    if view {
        output["view"] = json!({"fields":{"id":true},"maxBytes":1024});
    }
    serde_yaml::to_string(
        &json!({"schemaVersion":"ledger.task-registry.v1","tasks":{"read":{
            "model":"coder","prompt":"work.md","tools":["t.read"],"completion":"tool-result",
            "outputContract":{"outputs":{"data":output},"requireArtifactComplete":["original"]}
        }}}),
    )
    .unwrap()
}

fn run(
    payload: Value,
    schema: Option<Value>,
    view: bool,
    override_output: bool,
    jsonl: bool,
) -> (RunStatus, Value, Vec<Value>, usize) {
    let base = common::tmp_dir("artifact-output-schema");
    std::fs::write(base.join("work.md"), "Perform one read.").unwrap();
    let run = RunDir::under(&base);
    let calls = Arc::new(Mutex::new(0));
    let mut plan: Value = serde_yaml::from_str(PLAN).unwrap();
    if override_output {
        plan["nodes"]["read"]["outputs"] = json!({"data":{"fromArtifact":"original","format":if jsonl {"jsonl"} else {"json"},"view":{"fields":{"id":true},"maxBytes":1024}}});
    }
    let registry = TaskRegistry::from_yaml_str(&registry(schema, view, jsonl), &base).unwrap();
    let mut executor = Executor::new(
        ExecutionPlan::from_yaml_str(&serde_yaml::to_string(&plan).unwrap()).unwrap(),
        registry,
        String::new(),
        SlotMap::new().with("coder", MockModel::scripted([CALL, CALL])),
        Gateway {
            run: run.clone(),
            payload: payload.clone(),
            calls: calls.clone(),
            jsonl,
        },
        run.clone(),
    )
    .unwrap();
    let status = executor.run_until_blocked().unwrap();
    let vars = executor.state().stack[0].vars.clone();
    let events = common::read_events(run.path());
    assert!(artifacts::is_complete(
        &run.artifacts_dir().join("original.json")
    ));
    if !jsonl {
        assert_eq!(
            serde_json::from_slice::<Value>(
                &std::fs::read(run.artifacts_dir().join("original.json")).unwrap()
            )
            .unwrap(),
            payload
        );
    }
    let count = *calls.lock().unwrap();
    std::fs::remove_dir_all(base).unwrap();
    (status, vars, events, count)
}

#[test]
fn nested_required_enum_pattern_reject_original_before_views_bindings_and_downstream() {
    for payload in [
        json!({"id":"private-instance","tag":"a","attributes":{}}),
        json!({"id":"private-instance","tag":"button","attributes":{"href":"#scope"}}),
        json!({"id":"private-instance","tag":"a","attributes":{"href":"%23scope"}}),
        json!({"id":"private-instance","tag":"a","attributes":{"href":"/other"}}),
        json!({"id":"private-instance","tag":"a","attributes":{"href":"#scope?x"}}),
    ] {
        for override_output in [false, true] {
            let (status, vars, events, count) = run(
                payload.clone(),
                Some(schema()),
                true,
                override_output,
                false,
            );
            assert!(matches!(status, RunStatus::Failed { .. }));
            assert_eq!(count, 1);
            assert!(vars.get("read").is_none());
            let error = events
                .iter()
                .find(|event| event["type"] == "error")
                .unwrap();
            assert_eq!(error["class"], "verification-failed");
            assert_eq!(
                error["detail"],
                "output 'data' artifact schema validation failed"
            );
            assert!(!error.to_string().contains("private-instance"));
        }
    }
}

#[test]
fn valid_large_original_is_checked_before_small_projection_and_old_flows_stay_valid() {
    let payload = json!({"id":"actual","tag":"a","attributes":{"href":"#scope_1-2"},"large":"x".repeat(300_000)});
    for override_output in [false, true] {
        let (status, vars, _, count) = run(
            payload.clone(),
            Some(schema()),
            true,
            override_output,
            false,
        );
        assert_eq!(status, RunStatus::Succeeded);
        assert_eq!(count, 2);
        assert_eq!(vars["read"]["outputs"]["data"], json!({"id":"actual"}));
    }
    assert_eq!(
        run(json!({"legacy":true}), None, false, false, false).0,
        RunStatus::Succeeded
    );
}

#[test]
fn jsonl_schema_checks_full_array_items() {
    let array_schema = json!({"type":"array","items":schema(),"minItems":1});
    let valid = json!({"id":"one","tag":"a","attributes":{"href":"#scope"}});
    assert_eq!(
        run(
            json!([valid]),
            Some(array_schema.clone()),
            false,
            false,
            true
        )
        .0,
        RunStatus::Succeeded
    );
    assert!(matches!(
        run(
            json!([valid,{"tag":"a"}]),
            Some(array_schema),
            true,
            false,
            true
        )
        .0,
        RunStatus::Failed { .. }
    ));
}

#[test]
fn invalid_schema_and_model_overrides_are_rejected_before_any_execution() {
    for invalid in [
        json!({"type":"nonsense"}),
        json!({"pattern":"["}),
        json!({"$ref":"https://invalid.example/private-schema"}),
        Value::Null,
        json!(false),
        json!({"description":"x".repeat(65536)}),
    ] {
        assert!(TaskRegistry::from_yaml_str(&registry(Some(invalid), false, false), ".").is_err());
    }
    let mut output =
        serde_json::from_str::<Value>(r#"{"from":"model","type":"prose","schema":{}}"#).unwrap();
    let mut invalid: Value = serde_yaml::from_str(&registry(None, false, false)).unwrap();
    invalid["tasks"]["read"]["outputContract"]["outputs"]["data"] = output.take();
    assert!(TaskRegistry::from_yaml_str(&serde_yaml::to_string(&invalid).unwrap(), ".").is_err());
    let mut registry_value: Value =
        serde_yaml::from_str(&registry(Some(schema()), false, false)).unwrap();
    registry_value["tasks"]["read"]["completion"] = json!("model-prose");
    registry_value["tasks"]["read"]["tools"] = json!([]);
    let registry =
        TaskRegistry::from_yaml_str(&serde_yaml::to_string(&registry_value).unwrap(), ".").unwrap();
    let mut plan: Value = serde_yaml::from_str(PLAN).unwrap();
    plan["nodes"]["read"]["outputs"] = json!({"data":{"from":"model"}});
    let plan = ExecutionPlan::from_yaml_str(&serde_yaml::to_string(&plan).unwrap()).unwrap();
    let error = trollm_core::validate::validate_with_registry(&plan, &registry)
        .unwrap_err()
        .to_string();
    assert!(error.contains("schema-bearing artifact output"));
}
