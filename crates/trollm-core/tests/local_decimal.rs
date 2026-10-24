//! Production executor and gateway with scripted coder calls; no inference/network.
mod common;

use serde_json::{json, Value};
use trollm_core::{ExecutionPlan, Executor, RunDir, RunStatus, SlotMap, TaskRegistry};
use trollm_mcp::{McpGateway, PolicySet};
use trollm_models::MockModel;

fn run(responses: &[String], grant: bool) -> (RunStatus, Value, Vec<Value>) {
    let base = common::tmp_dir("local-decimal");
    let run = RunDir::under(&base);
    std::fs::write(
        base.join("prompt.md"),
        "Make the granted call using bound evidence.",
    )
    .unwrap();
    let plan = ExecutionPlan::from_yaml_str(
        r#"
schemaVersion: ledger.execution-plan.v1
id: local-decimal
version: 1
input: { promptTemplate: "{{ user_input }}" }
limits: { maxSteps: 2, maxReasonerCalls: 0, maxTokensTotal: 4096, timeoutS: 30, artifactMaxMb: 1 }
onLimit: abort
models: { coder: { ctx: 4096 } }
mcp:
  servers: []
  policies:
    local.extract_decimal: { resultArtifact: extraction }
    local.calculate: { resultArtifact: calculation }
entryStep: extract
nodes:
  extract: { kind: task, taskRef: extract, onVerified: compare }
  compare: { kind: task, taskRef: compare, onVerified: done }
  done: { kind: end, outcome: succeeded }
"#,
    )
    .unwrap();
    let registry = r#"
schemaVersion: ledger.task-registry.v1
id: local-decimal
version: 1
tasks:
  extract:
    model: coder
    prompt: prompt.md
    tools: [local.extract_decimal]
    completion: tool-result
    loop: { maxSteps: 1 }
    toolArguments:
      local.extract_decimal:
        fromInput: { primary: source }
        fixed: { secondary: "", rules: { prefix: "$", fractionDigits: 2 } }
    outputContract:
      outputs:
        evidence: { fromArtifact: extraction, format: json, type: object }
      requireArtifactComplete: [extraction]
  compare:
    model: coder
    prompt: prompt.md
    tools: [local.calculate]
    completion: tool-result
    loop: { maxSteps: 1 }
    toolArguments:
      local.calculate:
        fromVars: { left: extract.outputs.evidence.decimal }
        fixed: { operation: compare, right: "3" }
    outputContract:
      outputs:
        calculation: { fromArtifact: calculation, format: json, type: object }
      requireArtifactComplete: [calculation]
"#;
    let registry = if grant {
        registry.to_owned()
    } else {
        // An undeclared tool has no binding entry either; this isolates runtime grant denial.
        registry
            .replace("tools: [local.extract_decimal]", "tools: [local.calculate]")
            .replace(
                "local.extract_decimal:\n        fromInput",
                "local.calculate:\n        fromInput",
            )
    };
    let registry = TaskRegistry::from_yaml_str(&registry, &base).unwrap();
    let policy =
        PolicySet::from_yaml_value(&serde_yaml::to_value(&plan.mcp.policies).unwrap()).unwrap();
    let gateway = McpGateway::new(
        vec![],
        policy,
        base.join("cache"),
        run.artifacts_dir(),
        None,
    )
    .unwrap();
    let mut executor = Executor::new(
        plan,
        registry,
        json!({"source":"Observed $2.90"}).to_string(),
        SlotMap::new().with("coder", MockModel::scripted(responses.iter().cloned())),
        gateway,
        run.clone(),
    )
    .unwrap();
    let status = executor.run_until_blocked().unwrap();
    let vars = serde_json::to_value(&executor.state().stack[0].vars).unwrap();
    let events = common::read_events(run.path());
    if status == RunStatus::Succeeded {
        let artifact: Value = serde_json::from_slice(
            &std::fs::read(run.artifacts_dir().join("extraction.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(artifact, vars["extract"]["outputs"]["evidence"]);
    }
    drop(executor);
    std::fs::remove_dir_all(base).unwrap();
    (status, vars, events)
}

fn call(tool: &str, args: Value) -> String {
    format!(
        "<tool_call>{}</tool_call>",
        json!({"server":"local","tool":tool,"args":args})
    )
}

#[test]
fn granted_extraction_binds_exact_original_result_into_calculator() {
    let (status, vars, events) = run(
        &[
            call("extract_decimal", json!({})),
            call("calculate", json!({})),
        ],
        true,
    );
    assert_eq!(status, RunStatus::Succeeded);
    assert_eq!(vars["extract"]["outputs"]["evidence"]["decimal"], "2.9");
    assert_eq!(
        vars["extract"]["outputs"]["evidence"]["sources"][1]["status"],
        "unmatched"
    );
    assert_eq!(vars["compare"]["outputs"]["calculation"]["left"], "2.9");
    assert_eq!(vars["compare"]["outputs"]["calculation"]["result"], "less");
    assert_eq!(
        events
            .iter()
            .filter(|event| event["type"] == "tool_call_result")
            .count(),
        2
    );
}

#[test]
fn missing_grant_and_conflicting_bound_source_cannot_publish_extraction() {
    for (grant, args) in [
        (false, json!({})),
        (true, json!({"primary":"Forged $1.00"})),
        (true, json!({"rules":{"prefix":"$","fractionDigits":3}})),
    ] {
        let (status, _, events) = run(&[call("extract_decimal", args)], grant);
        assert!(matches!(status, RunStatus::Failed { .. }));
        assert!(!events
            .iter()
            .any(|event| event["type"] == "tool_call_result"));
    }
}
