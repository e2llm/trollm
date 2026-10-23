//! Real executor + builtin gateway; model extraction/advice are scripted.
//! No network, credentials, weights or claim of model factual accuracy.

mod common;

use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use trollm_core::{ExecutionPlan, Executor, RunDir, RunStatus, SlotMap, TaskRegistry};
use trollm_mcp::{McpGateway, PolicySet};
use trollm_models::{GenerateRequest, GenerateResponse, MockModel, ModelSlot};

const INPUT: &str = "My cart has items at 39.99 and 40.10. My budget is 80.";
const ADD: &str = r#"<tool_call>{"server":"local","tool":"calculate","args":{"left":"39.99","right":"40.10"}}</tool_call>"#;
const COMPARE: &str = r#"<tool_call>{"server":"local","tool":"calculate","args":{}}</tool_call>"#;
const ADVICE: &str = "The selected items total 80.09, above your budget of 80. Consider a cheaper item; no purchase is authorized.";
const REPORT: &str = "39.99 + 40.10 = 80.09, which exceeds 80. Consider a cheaper item.";

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

struct Fixture {
    base: PathBuf,
    run: RunDir,
    coder_prompts: Arc<Mutex<Vec<String>>>,
    reasoner_prompts: Arc<Mutex<Vec<String>>>,
}

impl Fixture {
    fn new() -> Self {
        let base = common::tmp_dir("prose-calculation");
        Self {
            run: RunDir::under(&base),
            base,
            coder_prompts: Arc::default(),
            reasoner_prompts: Arc::default(),
        }
    }

    fn executor(&self, responses: &[&str]) -> Executor<RecordingModel, McpGateway> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let plan =
            ExecutionPlan::from_yaml_file(&root.join("flows/examples/prose-calculation.flow.yaml"))
                .unwrap();
        assert!(plan.mcp.servers.is_empty());
        let registry =
            TaskRegistry::from_yaml_file(&root.join("flows/tasks/prose-calculation.tasks.yaml"))
                .unwrap();
        assert!(registry.tasks["advise-cart"].tools.is_empty());
        assert!(registry.tasks["report-cart"].tools.is_empty());
        let policies =
            PolicySet::from_yaml_value(&serde_yaml::to_value(&plan.mcp.policies).unwrap()).unwrap();
        let gateway = McpGateway::new(
            vec![],
            policies,
            self.base.join("cache"),
            self.run.artifacts_dir(),
            None,
        )
        .unwrap();
        Executor::new(
            plan,
            registry,
            INPUT.to_string(),
            SlotMap::new()
                .with(
                    "coder",
                    RecordingModel {
                        model: MockModel::scripted(responses.iter().copied()),
                        prompts: self.coder_prompts.clone(),
                    },
                )
                .with(
                    "reasoner",
                    RecordingModel {
                        model: MockModel::scripted([ADVICE]),
                        prompts: self.reasoner_prompts.clone(),
                    },
                ),
            gateway,
            self.run.clone(),
        )
        .unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.base).unwrap();
    }
}

#[test]
fn exact_calculations_and_original_prose_reach_tool_free_advice_then_coder() {
    let fixture = Fixture::new();
    let mut executor = fixture.executor(&[ADD, COMPARE, REPORT]);
    assert_eq!(executor.run_until_blocked().unwrap(), RunStatus::Succeeded);
    let vars = &executor.state().stack[0].vars;
    let sum = json!({
        "format":"trollm.local.calculate.v1", "operation":"add",
        "left":"39.99", "right":"40.10", "result":"80.09"
    });
    let comparison = json!({
        "format":"trollm.local.calculate.v1", "operation":"compare",
        "left":"80.09", "right":"80", "result":"greater"
    });
    assert_eq!(vars["sum"]["outputs"]["calculation"], sum);
    assert_eq!(vars["compare"]["outputs"]["calculation"], comparison);
    assert_eq!(vars["advise"]["outputs"]["advice"], ADVICE);
    assert_eq!(vars["report"]["outputs"]["report"], REPORT);

    // The shared artifact now contains compare; the bound sum stays distinct.
    let artifact: Value = serde_json::from_slice(
        &std::fs::read(fixture.run.artifacts_dir().join("calculation.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(artifact, comparison);
    let reasoner = fixture.reasoner_prompts.lock().unwrap();
    assert_eq!(reasoner.len(), 1);
    for exact in [INPUT.to_string(), sum.to_string(), comparison.to_string()] {
        assert!(reasoner[0].contains(&exact), "missing exact input: {exact}");
    }
    assert!(!reasoner[0].contains("{{"));
    let coder = fixture.coder_prompts.lock().unwrap();
    assert_eq!(coder.len(), 3, "each calculation completes after one call");
    assert!(coder[0].contains(INPUT));
    for exact in [
        INPUT.to_string(),
        sum.to_string(),
        comparison.to_string(),
        ADVICE.to_string(),
    ] {
        assert!(coder[2].contains(&exact));
    }

    let events = common::read_events(fixture.run.path());
    assert_eq!(
        events.iter().filter(|e| e["type"] == "model_call").count(),
        4
    );
    let results: Vec<_> = events
        .iter()
        .filter(|e| e["type"] == "tool_call_result")
        .collect();
    assert_eq!(results.len(), 2);
    assert!(results
        .iter()
        .all(|e| e["server"] == "local" && e["tool"] == "calculate"));
    assert!(
        !events.iter().any(|e| e["type"] == "tool_call_intent"),
        "pure arithmetic has no mutation intent"
    );
}

#[test]
fn missing_multiple_denied_or_conflicting_calculator_calls_fail_without_artifact() {
    for response in [
        "The total is 80.09.".to_string(),
        format!("{ADD}\n{ADD}"),
        ADD.replace("calculate", "unknown"),
        ADD.replace(
            r#""left":"39.99""#,
            r#""operation":"subtract","left":"39.99""#,
        ),
    ] {
        let fixture = Fixture::new();
        let mut executor = fixture.executor(&[&response]);
        assert!(matches!(
            executor.run_until_blocked().unwrap(),
            RunStatus::Failed { .. }
        ));
        assert_eq!(fixture.coder_prompts.lock().unwrap().len(), 1);
        assert!(fixture.reasoner_prompts.lock().unwrap().is_empty());
        assert!(!fixture
            .run
            .artifacts_dir()
            .join("calculation.json")
            .exists());
        let events = common::read_events(fixture.run.path());
        assert!(!events.iter().any(|e| matches!(
            e["type"].as_str(),
            Some("tool_call_intent" | "tool_call_result")
        )));
    }
}

#[test]
fn conflicting_bound_sum_or_budget_cannot_reach_advice() {
    for args in [r#"{"left":"79.99"}"#, r#"{"right":"81"}"#] {
        let response = COMPARE.replace("{}", args);
        let fixture = Fixture::new();
        let mut executor = fixture.executor(&[ADD, &response]);
        assert!(matches!(
            executor.run_until_blocked().unwrap(),
            RunStatus::Failed { .. }
        ));
        assert_eq!(fixture.coder_prompts.lock().unwrap().len(), 2);
        assert!(fixture.reasoner_prompts.lock().unwrap().is_empty());
        let events = common::read_events(fixture.run.path());
        assert_eq!(
            events
                .iter()
                .filter(|e| e["type"] == "tool_call_result")
                .count(),
            1
        );
    }
}

#[test]
fn reasoner_cannot_be_granted_even_the_builtin_calculator() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let path = root.join("flows/tasks/prose-calculation.tasks.yaml");
    let registry = std::fs::read_to_string(&path).unwrap().replace(
        "prompt: prompts/prose-calculation-advise.md\n    tools: []",
        "prompt: prompts/prose-calculation-advise.md\n    tools: [local.calculate]",
    );
    let error = TaskRegistry::from_yaml_str(&registry, path.parent().unwrap()).unwrap_err();
    assert!(error
        .to_string()
        .contains("reasoner model slot must not grant tools"));
}
