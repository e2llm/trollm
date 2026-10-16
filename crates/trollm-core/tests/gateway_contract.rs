//! Exercise the production MCP artifact producer against the core consumer.

#[path = "../../trollm-mcp/tests/common/mock.rs"]
mod mock;

use mock::{MockMcpServer, TempDir};
use serde_json::{json, Value};
use trollm_core::{ExecutionPlan, Executor, RunDir, RunStatus, SlotMap, TaskRegistry};
use trollm_mcp::{McpGateway, PolicySet, Provenance, ServerConfig, ToolCall, ToolGateway};
use trollm_models::MockModel;

fn run_listing(query: &str, warm_cache: bool) {
    let server = MockMcpServer::start();
    let dirs = TempDir::new("core-gateway-contract");
    let run_dir = RunDir::under(dirs.path());
    let plan = ExecutionPlan::from_yaml_str(&format!(
        r#"
schemaVersion: ledger.execution-plan.v1
id: real-listing
version: 1
sourceBaseline: {{ sha256: "{}" }}
input: {{ promptTemplate: "list issues" }}
limits: {{ maxSteps: 10, maxReasonerCalls: 0, maxTokensTotal: 10000, timeoutS: 60, artifactMaxMb: 5 }}
onLimit: abort
models: {{ coder: {{ ctx: 8192 }} }}
mcp: {{ servers: [], policies: {{}} }}
entryStep: setup
nodes:
  setup: {{ kind: task, taskRef: list, onVerified: success, onError: {{ then: failure }} }}
  success: {{ kind: end, outcome: succeeded }}
  failure: {{ kind: end, outcome: failed }}
"#,
        "0".repeat(64)
    ))
    .unwrap();
    let registry = TaskRegistry::from_yaml_str(
        r#"
schemaVersion: ledger.task-registry.v1
tasks:
  list:
    model: coder
    prompt: list.md
    tools: [tracker.list_issues]
    loop: { maxSteps: 2 }
    outputContract:
      outputs:
        items: { fromArtifact: tracker.list_issues.latest, format: jsonl, type: list }
      requireArtifactComplete: [tracker.list_issues.latest]
"#,
        dirs.path(),
    )
    .unwrap();
    std::fs::write(dirs.path().join("list.md"), "List all issues").unwrap();
    let policies = PolicySet::from_yaml_str(
        r#"
tracker.list_issues:
  class: read
  cache: { key: [query], ttlS: 900 }
  paginate: { param: cursor, deliver: first_page, backgroundFetch: all }
  shape: { strip: [body], toFile: true }
"#,
    )
    .unwrap();
    let mut gateway = McpGateway::new(
        vec![ServerConfig::for_local_test("tracker", server.url.clone())],
        policies,
        dirs.path().join("cache"),
        run_dir.artifacts_dir(),
        None,
    )
    .unwrap();
    let call = ToolCall {
        server: "tracker".into(),
        tool: "list_issues".into(),
        args: json!({"query": query}),
    };
    if warm_cache {
        let first = gateway.call(&call).unwrap();
        assert_eq!(first.provenance, Provenance::Live);
        assert_eq!(server.count("call:list_issues"), 3);
        // Remove the initial publication so success must republish from cache.
        std::fs::remove_dir_all(run_dir.artifacts_dir()).unwrap();
    }
    let fence = format!(
        "<tool_call>{}</tool_call>",
        serde_json::to_string(&call).unwrap()
    );
    let mut exec = Executor::new(
        plan,
        registry,
        String::new(),
        SlotMap::new().with("coder", MockModel::scripted([fence, "Listed".into()])),
        gateway,
        run_dir.clone(),
    )
    .unwrap();
    let status = exec.run_until_blocked().unwrap();
    let events: Vec<Value> = std::fs::read_to_string(run_dir.events_path())
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    if query == "cycle" {
        assert!(matches!(status, RunStatus::Failed { .. }));
        assert!(exec.state().stack[0].vars.get("setup").is_none());
        assert!(events
            .iter()
            .any(|event| event["type"] == "error" && event["class"] == "mcp-error"));
        assert!(events.iter().any(|event| event["type"] == "transition"
            && event["to"] == "failure"
            && event["key"] == "onError"));
    } else {
        assert_eq!(status, RunStatus::Succeeded);
        let items = exec.state().stack[0].vars["setup"]["outputs"]["items"]
            .as_array()
            .unwrap();
        assert_eq!(
            items
                .iter()
                .map(|item| item["id"].as_u64().unwrap())
                .collect::<Vec<_>>(),
            vec![1, 2, 3, 4, 5, 6]
        );
        assert!(
            items.iter().all(|item| item["body"].is_string()),
            "worklist must retain the full records from all pages"
        );
        assert_eq!(
            server.count("call:list_issues"),
            3,
            "a warm cache must not fetch any pages again"
        );
        let expected = if warm_cache { "Cache" } else { "Live" };
        assert!(events
            .iter()
            .any(|event| event["type"] == "tool_call_result" && event["provenance"] == expected));
    }
}

#[test]
fn real_gateway_publishes_complete_worklist_for_cold_and_warm_core_execution() {
    run_listing("open", false);
    run_listing("open", true);
}

#[test]
fn real_gateway_pagination_cycle_routes_core_to_on_error() {
    run_listing("cycle", false);
}
