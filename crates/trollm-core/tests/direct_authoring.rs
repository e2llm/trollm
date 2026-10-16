//! Executes the directly authored summary reference with a scripted MockModel.
//! This fixture checks wiring and output binding, not summary quality or inference.

mod common;

use std::path::Path;
use std::sync::{Arc, Mutex};
use trollm_core::{ExecutionPlan, Executor, RunDir, RunStatus, SlotMap, TaskRegistry};
use trollm_models::{GenerateRequest, GenerateResponse, MockModel, ModelSlot};

struct RecordingModel {
    model: MockModel,
    prompts: Arc<Mutex<Vec<String>>>,
}

impl ModelSlot for RecordingModel {
    fn generate(&mut self, request: GenerateRequest) -> anyhow::Result<GenerateResponse> {
        self.prompts.lock().unwrap().push(request.prompt.clone());
        self.model.generate(request)
    }
}

#[test]
fn directly_authored_summary_runs_without_tools_or_recording() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let plan =
        ExecutionPlan::from_yaml_file(&root.join("flows/examples/minimal-summary.flow.yaml"))
            .expect("directly authored plan loads without sourceBaseline");
    assert!(plan.source_baseline.is_none());
    let registry =
        TaskRegistry::from_yaml_file(&root.join("flows/tasks/minimal-summary.tasks.yaml"))
            .expect("summary registry and real prompt load");
    assert!(registry.source_baseline.is_none());

    let base = common::tmp_dir("direct-authoring");
    let run = RunDir::under(&base);
    let prompts = Arc::new(Mutex::new(Vec::new()));
    // Scripted fixture response: no real model or factual verification occurs.
    let report = "The team moved the meeting to Friday and will send an agenda.";
    let input = "The meeting is now on Friday. The team will send an agenda beforehand.";
    let model = RecordingModel {
        model: MockModel::scripted([report]),
        prompts: prompts.clone(),
    };
    let mut executor = Executor::new(
        plan,
        registry,
        input.to_string(),
        SlotMap::new().with("coder", model),
        common::ScriptGateway::new(run.artifacts_dir()),
        run.clone(),
    )
    .expect("example validates and constructs");

    assert_eq!(executor.run_until_blocked().unwrap(), RunStatus::Succeeded);
    assert_eq!(
        executor.state().stack[0].vars["summarize"]["outputs"]["report"],
        report
    );
    let requests = prompts.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert!(requests[0].contains(input), "user_input reaches the model");
    assert!(!requests[0].contains("{{ user_input }}"));
    let events = common::read_events(run.path());
    assert!(!events.iter().any(|event| {
        matches!(
            event["type"].as_str(),
            Some("tool_call_intent" | "tool_call_result")
        )
    }));
    drop(executor);
    std::fs::remove_dir_all(base).unwrap();
}
