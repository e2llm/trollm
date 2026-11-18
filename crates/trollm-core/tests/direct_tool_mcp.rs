//! Direct execution through the production gateway and loopback MCP transport.
//! The local server supplies fixture data; no external service or model runs.

#[path = "../../trollm-mcp/tests/common/mock.rs"]
#[allow(dead_code)] // This binary uses the shared fixture's custom-catalog entry point.
mod mock;

use mock::{MockMcpServer, TempDir};
use serde_json::{json, Value};
use std::path::Path;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use trollm_core::{artifacts, ExecutionPlan, Executor, RunDir, RunStatus, SlotMap, TaskRegistry};
use trollm_mcp::{EventSink, McpGateway, PolicySet, ProxyEvent, ServerConfig};
use trollm_models::MockModel;

fn events(path: &Path) -> Vec<Value> {
    std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

#[test]
fn direct_read_and_journaled_mutation_cross_the_mcp_wire_without_models() {
    let server = MockMcpServer::start_with_tools_response(json!({"result":{"tools":[
        {"name":"get_issue","inputSchema":{
            "type":"object","additionalProperties":false,"required":["id"],
            "properties":{"id":{"type":"integer","minimum":1}}
        }},
        {"name":"close_issue","inputSchema":{
            "type":"object","additionalProperties":false,"required":["id","confirmed"],
            "properties":{"id":{"type":"integer","minimum":1},"confirmed":{"type":"boolean"}}
        }}
    ]}}));
    let dirs = TempDir::new("core-direct-mcp");
    let run = RunDir::under(dirs.path());
    let plan = ExecutionPlan::from_yaml_str(
        r#"
schemaVersion: ledger.execution-plan.v1
id: direct-mcp
version: 1
limits: {maxSteps: 2, maxReasonerCalls: 0, maxTokensTotal: 0, timeoutS: 60, artifactMaxMb: 1}
onLimit: abort
models: {}
mcp:
  servers: []
  policies:
    tracker.get_issue: {class: read, resultArtifact: default-issue}
    tracker.close_issue: {class: mutation, passthrough: true, resultArtifact: mutation-result}
entryStep: read
nodes:
  read: {kind: task, taskRef: read, onVerified: close}
  close: {kind: task, taskRef: close, onVerified: done}
  done: {kind: end, outcome: succeeded}
"#,
    )
    .unwrap();
    let registry = TaskRegistry::from_yaml_str(
        r#"
schemaVersion: ledger.task-registry.v1
tasks:
  read:
    execution: tool
    completion: tool-result
    tools: [tracker.get_issue]
    resultArtifact: observed-issue
    toolArguments:
      tracker.get_issue:
        modelFields: []
        fromInput: {id: issue_id}
    outputContract:
      outputs:
        issue: {fromArtifact: observed-issue, format: json, type: object}
      requireArtifactComplete: [observed-issue]
  close:
    execution: tool
    completion: tool-result
    tools: [tracker.close_issue]
    toolArguments:
      tracker.close_issue:
        modelFields: []
        fixed: {confirmed: true}
        fromVars: {id: read.outputs.issue.id}
    mutations: {idempotency: intent-log, onUncertain: fail-run}
    outputContract:
      outputs:
        receipt: {fromArtifact: mutation-result, format: json, type: object}
      requireArtifactComplete: [mutation-result]
"#,
        dirs.path(),
    )
    .unwrap();
    let policies =
        PolicySet::from_yaml_value(&serde_yaml::to_value(&plan.mcp.policies).unwrap()).unwrap();

    // Check the actual journal at the gateway's pre-send mutation boundary.
    let journal = run.events_path();
    let wire_counts = server.counts.clone();
    let saw_retained_intent = Arc::new(AtomicBool::new(false));
    let observed_intent = saw_retained_intent.clone();
    let sink: EventSink =
        Box::new(move |event| {
            if let ProxyEvent::MutationPassthrough { server, tool, .. } = event {
                assert_eq!((server.as_str(), tool.as_str()), ("tracker", "close_issue"));
                assert_eq!(
                    wire_counts
                        .lock()
                        .unwrap()
                        .get("call:close_issue")
                        .copied()
                        .unwrap_or(0),
                    0,
                    "the write must not have reached the MCP fixture before its intent"
                );
                let records = events(&journal);
                let intent = records
                    .iter()
                    .find(|event| event["type"] == "tool_call_intent")
                    .unwrap();
                assert_eq!(intent["args"], json!({"id":7,"confirmed":true}));
                assert!(!records
                    .iter()
                    .any(|event| event["type"] == "tool_call_result"
                        && event["tool"] == "close_issue"));
                observed_intent.store(true, Ordering::SeqCst);
            }
            Ok(())
        });
    let gateway = McpGateway::new(
        vec![ServerConfig::for_local_test("tracker", &server.url)],
        policies,
        dirs.path().join("cache"),
        run.artifacts_dir(),
        Some(sink),
    )
    .unwrap();
    let mut executor = Executor::new(
        plan,
        registry,
        json!({"issue_id":7}).to_string(),
        SlotMap::<MockModel>::new(),
        gateway,
        run.clone(),
    )
    .unwrap();

    assert_eq!(executor.run_until_blocked().unwrap(), RunStatus::Succeeded);
    assert!(saw_retained_intent.load(Ordering::SeqCst));
    assert_eq!(server.count("initialize"), 1);
    assert_eq!(server.count("list"), 1);
    assert_eq!(server.count("call:get_issue"), 1);
    assert_eq!(server.count("call:close_issue"), 1);
    let original = json!({"id":7,"title":"Issue 7","body":"live body"});
    let receipt = json!({"ok":true,"id":7});
    let vars = &executor.state().stack[0].vars;
    assert_eq!(vars["read"]["outputs"]["issue"], original);
    assert_eq!(vars["close"]["outputs"]["receipt"], receipt);
    for (name, expected) in [("observed-issue", original), ("mutation-result", receipt)] {
        let path = run.artifacts_dir().join(format!("{name}.json"));
        assert!(artifacts::is_complete(&path));
        assert_eq!(
            serde_json::from_slice::<Value>(&std::fs::read(path).unwrap()).unwrap(),
            expected
        );
    }
    assert_eq!(executor.state().counters.steps, 2);
    assert_eq!(executor.state().counters.tokens, 0);
    assert_eq!(executor.state().counters.reasoner_calls, 0);
    let records = events(&run.events_path());
    assert_eq!(
        records
            .iter()
            .filter(|event| event["type"] == "direct_tool_step")
            .count(),
        2
    );
    assert!(!records.iter().any(|event| event["type"] == "model_call"));
    assert_eq!(
        records
            .iter()
            .filter(|event| event["type"] == "tool_call_result")
            .count(),
        2
    );
    assert_eq!(
        records
            .iter()
            .filter(|event| event["type"] == "tool_call_intent")
            .count(),
        1
    );
    let intent = records
        .iter()
        .find(|event| event["type"] == "tool_call_intent")
        .unwrap();
    let result = records
        .iter()
        .find(|event| event["type"] == "tool_call_result" && event["tool"] == "close_issue")
        .unwrap();
    assert_eq!(result["intentSeq"], intent["seq"]);
    assert_eq!(result["callContext"], intent["callContext"]);
    assert_eq!(result["provenance"], "Live");
    assert!(intent["seq"].as_u64().unwrap() < result["seq"].as_u64().unwrap());
}
