//! Executable teaching flow: scripted models, real local tools and original artifacts.
mod common;

use serde_json::{json, Value};
use std::path::Path;
use std::sync::{Arc, Mutex};
use trollm_core::{ExecutionPlan, Executor, RunDir, RunStatus, SlotMap, TaskRegistry};
use trollm_mcp::{McpGateway, PolicySet};
use trollm_models::{GenerateRequest, GenerateResponse, MockModel, ModelSlot};

struct Recorder {
    model: MockModel,
    requests: Arc<Mutex<Vec<GenerateRequest>>>,
}
impl ModelSlot for Recorder {
    fn generate(&mut self, request: GenerateRequest) -> anyhow::Result<GenerateResponse> {
        self.requests.lock().unwrap().push(request.clone());
        self.model.generate(request)
    }
}

fn run(records: Value, answer: &str) -> (RunStatus, Value, Vec<GenerateRequest>) {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let plan = ExecutionPlan::from_yaml_file(&root.join("flows/examples/record-lookup.flow.yaml"))
        .unwrap();
    let mut registry =
        TaskRegistry::from_yaml_file(&root.join("flows/tasks/record-lookup.tasks.yaml")).unwrap();
    registry
        .tasks
        .get_mut("lookup-observe")
        .unwrap()
        .tool_arguments
        .as_mut()
        .unwrap()
        .get_mut("local.filter_records")
        .unwrap()
        .fixed
        .insert("items".into(), records);
    let base = common::tmp_dir("record-lookup");
    let dir = RunDir::under(&base);
    let policies =
        PolicySet::from_yaml_value(&serde_yaml::to_value(&plan.mcp.policies).unwrap()).unwrap();
    let gateway = McpGateway::new(
        vec![],
        policies,
        base.join("cache"),
        dir.artifacts_dir(),
        None,
    )
    .unwrap();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let observe = r#"<tool_call>{"server":"local","tool":"filter_records","args":{}}</tool_call>"#;
    let mut exec = Executor::new(
        plan,
        registry,
        "Find Printer offline".into(),
        SlotMap::new()
            .with(
                "coder",
                Recorder {
                    model: MockModel::scripted([observe, answer]),
                    requests: requests.clone(),
                },
            )
            .with(
                "reasoner",
                Recorder {
                    model: MockModel::scripted([
                        "This is the selected original; its current state is unverified.",
                    ]),
                    requests: requests.clone(),
                },
            ),
        gateway,
        dir,
    )
    .unwrap();
    let status = exec.run_until_blocked().unwrap();
    let vars = exec.state().stack[0].vars.clone();
    let recorded = requests.lock().unwrap().clone();
    drop(exec);
    std::fs::remove_dir_all(base).unwrap();
    (status, vars, recorded)
}

const SELECT: &str = r#"<tool_call>{"server":"local","tool":"select_unique","args":{"equals":"Printer offline"}}</tool_call>"#;

#[test]
fn readable_reference_preserves_identity_and_full_evidence_under_reordering() {
    let original = json!({"id":"changed-opaque-id","title":"Printer offline","label":"Printer offline","description":"Original detail not in the selection view"});
    for records in [
        json!([original, {"id":"other","title":"Reset account"}]),
        json!([{"id":"other","title":"Reset account"},original]),
    ] {
        let (status, vars, requests) = run(records, SELECT);
        assert_eq!(status, RunStatus::Succeeded);
        assert_eq!(
            vars["select"]["outputs"]["data"]["selected"]["selected"],
            original
        );
        assert_eq!(requests.len(), 3);
        assert!(!requests[1].prompt.contains("Original detail not in"));
        assert!(requests[2]
            .prompt
            .contains("Original detail not in the selection view"));
        assert!(requests[2].prompt.contains("changed-opaque-id"));
    }
}

#[test]
fn missing_or_ambiguous_reference_does_not_reach_advice() {
    let record = json!({"id":"one","title":"Printer offline"});
    for records in [
        json!([]),
        json!([{"id":"different","title":"Printer online"}]),
        json!([record, record]),
    ] {
        let (status, vars, requests) = run(records, SELECT);
        assert!(matches!(status, RunStatus::Paused { .. }));
        assert!(vars.get("select").is_none());
        assert_eq!(requests.len(), 2);
    }
}

#[test]
fn unsupported_choice_and_bound_argument_override_never_publish_a_selection() {
    let records = json!([{"id":"original","title":"Printer offline"}]);
    let decline = r#"<tool_call>{"server":"flow","tool":"choose","args":{"route":"no_selection"}}</tool_call>"#;
    let (status, vars, requests) = run(records.clone(), decline);
    assert!(matches!(status, RunStatus::Paused { .. }));
    assert!(vars.get("select").is_none());
    assert_eq!(requests.len(), 2);
    let override_call = SELECT.replace("\"equals\":", "\"field\":\"/selected/id\",\"equals\":");
    let (status, vars, _) = run(records, &override_call);
    assert!(matches!(status, RunStatus::Paused { .. }));
    assert!(vars.get("select").is_none());
}
