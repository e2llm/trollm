//! Actual local-calculator starter with scripted models and the real gateway.
//! This exercises flow wiring, not real-model operand selection or Android.

mod common;

use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use trollm_core::{ExecutionPlan, Executor, RunDir, RunStatus, SlotMap, TaskRegistry};
use trollm_mcp::{McpGateway, PolicySet};
use trollm_models::{GenerateRequest, GenerateResponse, MockModel, ModelSlot};

const TEXT: &str = "My cart has items at 39.99 and 40.10.";
const ADD: &str = r#"<tool_call>{"server":"local","tool":"calculate","args":{"left":"39.99","right":"40.10"}}</tool_call>"#;
const COMPARE: &str = r#"<tool_call>{"server":"local","tool":"calculate","args":{}}</tool_call>"#;
const ADVICE: &str = "Use the exact total and supplied budget; no purchase is authorized.";
const REPORT: &str = "The total is 80.09. Check the supplied budget before deciding.";

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
    coder: Arc<Mutex<Vec<String>>>,
    reasoner: Arc<Mutex<Vec<String>>>,
}

impl Fixture {
    fn new() -> Self {
        let base = common::tmp_dir("local-starter");
        Self {
            run: RunDir::under(&base),
            base,
            coder: Arc::default(),
            reasoner: Arc::default(),
        }
    }

    fn executor(&self, budget: &str, comparison: &str) -> Executor<RecordingModel, McpGateway> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../flows/starters/check-total");
        let plan = ExecutionPlan::from_yaml_file(&root.join("check-total.flow.yaml")).unwrap();
        let registry = TaskRegistry::from_yaml_file(&root.join("check-total.tasks.yaml")).unwrap();
        assert!(plan.mcp.servers.is_empty());
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
            json!({"text": TEXT, "budget": budget}).to_string(),
            SlotMap::new()
                .with(
                    "coder",
                    RecordingModel {
                        model: MockModel::scripted([ADD, comparison, REPORT]),
                        prompts: self.coder.clone(),
                    },
                )
                .with(
                    "reasoner",
                    RecordingModel {
                        model: MockModel::scripted([ADVICE]),
                        prompts: self.reasoner.clone(),
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
fn supplied_budget_and_exact_evidence_reach_advice_and_report() {
    for (budget, relation) in [("75", "greater"), ("100", "less")] {
        let fixture = Fixture::new();
        let mut executor = fixture.executor(budget, COMPARE);
        assert_eq!(executor.run_until_blocked().unwrap(), RunStatus::Succeeded);
        let vars = &executor.state().stack[0].vars;
        let sum = json!({
            "format":"trollm.local.calculate.v1", "operation":"add",
            "left":"39.99", "right":"40.10", "result":"80.09"
        });
        let comparison = json!({
            "format":"trollm.local.calculate.v1", "operation":"compare",
            "left":"80.09", "right":budget, "result":relation
        });
        assert_eq!(vars["sum"]["outputs"]["calculation"], sum);
        assert_eq!(vars["compare"]["outputs"]["calculation"], comparison);
        assert_eq!(vars["advise"]["outputs"]["advice"], ADVICE);
        assert_eq!(vars["report"]["outputs"]["report"], REPORT);
        let latest: Value = serde_json::from_slice(
            &std::fs::read(fixture.run.artifacts_dir().join("calculation.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(latest, comparison);
        let original = json!({"text": TEXT, "budget": budget}).to_string();
        let reasoner = fixture.reasoner.lock().unwrap();
        let coder = fixture.coder.lock().unwrap();
        assert_eq!(reasoner.len(), 1);
        assert_eq!(coder.len(), 3);
        for exact in [original, sum.to_string(), comparison.to_string()] {
            assert!(reasoner[0].contains(&exact), "missing evidence: {exact}");
            assert!(coder[2].contains(&exact), "missing evidence: {exact}");
        }
        assert!(coder[2].contains(ADVICE));
        let events = common::read_events(fixture.run.path());
        let results: Vec<_> = events
            .iter()
            .filter(|event| event["type"] == "tool_call_result")
            .collect();
        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|event| event["provenance"] == "Local"));
        assert!(!events
            .iter()
            .any(|event| event["type"] == "tool_call_intent"));
    }
}

#[test]
fn conflicting_or_invalid_budget_stops_before_advice() {
    let conflicting = COMPARE.replace("{}", r#"{"right":"80"}"#);
    for (budget, comparison) in [("75", conflicting.as_str()), ("75.0000001", COMPARE)] {
        let fixture = Fixture::new();
        let mut executor = fixture.executor(budget, comparison);
        assert!(matches!(
            executor.run_until_blocked().unwrap(),
            RunStatus::Failed { .. }
        ));
        assert!(fixture.reasoner.lock().unwrap().is_empty());
        assert_eq!(fixture.coder.lock().unwrap().len(), 2);
        let events = common::read_events(fixture.run.path());
        assert_eq!(
            events
                .iter()
                .filter(|event| event["type"] == "tool_call_result")
                .count(),
            1
        );
        let artifact: Value = serde_json::from_slice(
            &std::fs::read(fixture.run.artifacts_dir().join("calculation.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(artifact["operation"], "add");
        assert_eq!(artifact["result"], "80.09");
    }
}
