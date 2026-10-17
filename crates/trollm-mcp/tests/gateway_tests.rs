//! Policy-engine tests against the hand-rolled mock MCP server (§11).

mod common;

use common::{MockMcpServer, TempDir};
use serde_json::{json, Value};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use trollm_mcp::{
    CacheMeta, EventSink, McpGateway, PolicySet, Provenance, ServerConfig, ToolCall, ToolGateway,
};

#[test]
fn discovery_error_chain_exposes_rpc_code_without_remote_message_or_data() {
    let server = MockMcpServer::start_with_tools_response(json!({
        "error": {"code": -32603, "message": "secret-remote-message", "data": "secret-remote-data"}
    }));
    let dirs = TempDir::new("rpc-error-redaction");
    let gateway = gateway(&server, fixture_policies(), &dirs, None);
    let error = gateway
        .validate_call(&ToolCall {
            server: "tracker".into(),
            tool: "list_issues".into(),
            args: json!({"query": "test"}),
        })
        .unwrap_err();
    let chain = format!("{error:#}");
    assert!(chain.contains("MCP tool discovery failed"));
    assert!(chain.contains("MCP tools/list returned error code -32603"));
    assert!(!chain.contains("secret-remote-message"));
    assert!(!chain.contains("secret-remote-data"));
}

fn fixture_policies() -> PolicySet {
    PolicySet::from_yaml_str(
        r#"
tracker.list_issues:
  class: read
  cache: { key: [query], ttlS: 900 }
  paginate: { param: cursor, deliver: first_page, backgroundFetch: all }
  shape: { strip: [body], toFile: true }
tracker.get_issue:
  class: read
  serveFrom: { fetchedBy: tracker.list_issues, match: id }
  onMiss: fetch_live
tracker.close_issue:
  class: mutation
  passthrough: true
  timeoutS: 10
"#,
    )
    .expect("parse policies")
}

fn gateway(
    server: &MockMcpServer,
    policies: PolicySet,
    dirs: &TempDir,
    sink: Option<EventSink>,
) -> McpGateway {
    McpGateway::new(
        vec![ServerConfig::for_local_test("tracker", server.url.clone())],
        policies,
        dirs.path().join("cache"),
        dirs.path().join("artifacts"),
        sink,
    )
    .expect("build gateway")
}

fn call(server: &str, tool: &str, args: Value) -> ToolCall {
    ToolCall {
        server: server.into(),
        tool: tool.into(),
        args,
    }
}

fn read_result_artifact(dirs: &TempDir) -> Value {
    let path = dirs.path().join("artifacts/page.json");
    let meta: Value =
        serde_json::from_slice(&std::fs::read(path.with_file_name("page.json.meta.json")).unwrap())
            .unwrap();
    assert_eq!(meta["complete"], true);
    assert!(meta["fetchedAt"].as_u64().unwrap() > 0);
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

#[test]
fn result_artifact_rejects_unsafe_reserved_and_incompatible_policies() {
    for name in [
        "",
        ".",
        "..",
        "../page",
        "/page",
        "a/b",
        "a\\b",
        "page.json",
        "tool.latest",
        "node.snapshot",
        "page.meta",
        "_page",
        "a b",
        "café",
        &"a".repeat(65),
    ] {
        let policy = json!({"tracker.summarize": {"resultArtifact": name}});
        assert!(
            PolicySet::from_yaml_str(&policy.to_string()).is_err(),
            "{name}"
        );
    }
    for class in ["read", "mutation"] {
        let mut policy = json!({"class": class, "paginate": {"param": "cursor", "deliver": "first_page", "backgroundFetch": "all"}});
        policy["resultArtifact"] = json!("page");
        assert!(
            PolicySet::from_yaml_str(&json!({"tracker.summarize": policy}).to_string()).is_err()
        );
    }
    for name in ["page", "Page_2-raw", &"a".repeat(64)] {
        for class in ["read", "mutation"] {
            assert!(PolicySet::from_yaml_str(
                &json!({"tracker.summarize": {"class": class, "resultArtifact": name}}).to_string()
            )
            .is_ok());
        }
    }
    // Public policy fields cannot bypass validation at gateway construction.
    let mut policies =
        PolicySet::from_yaml_str("tracker.summarize: {resultArtifact: page}").unwrap();
    policies
        .policies
        .get_mut("tracker.summarize")
        .unwrap()
        .result_artifact = Some("../outside".into());
    let dirs = TempDir::new("result-policy");
    assert!(McpGateway::new(
        vec![],
        policies,
        dirs.path().join("cache"),
        dirs.path().join("artifacts"),
        None
    )
    .is_err());
}

#[test]
fn result_artifact_preserves_raw_json_before_shape_and_republishes_cache_hit() {
    let server = MockMcpServer::start();
    let dirs = TempDir::new("result-cache");
    let policies = PolicySet::from_yaml_str("tracker.get_issue: {resultArtifact: page, cache: {key: [id], ttlS: 900}, shape: {strip: [body], toFile: true}}").unwrap();
    let mut gw = gateway(&server, policies, &dirs, None);
    let c = call("tracker", "get_issue", json!({"id": 7}));
    let cold = gw.call(&c).unwrap();
    assert_eq!(cold.provenance, Provenance::Live);
    assert!(cold.content_for_model["body"].is_object());
    assert_eq!(
        read_result_artifact(&dirs),
        json!({"id": 7, "title": "Issue 7", "body": "live body"})
    );
    let path = dirs.path().join("artifacts/page.json");
    assert!(cold.artifacts.contains(&path));
    std::fs::remove_file(&path).unwrap();
    std::fs::remove_file(path.with_file_name("page.json.meta.json")).unwrap();
    let warm = gw.call(&c).unwrap();
    assert_eq!(warm.provenance, Provenance::Cache);
    assert!(warm.artifacts.contains(&path));
    assert_eq!(read_result_artifact(&dirs)["body"], "live body");
    assert_eq!(server.count("call:get_issue"), 1);
}

#[test]
fn result_artifact_replaces_success_but_tool_errors_do_not_publish() {
    let server = MockMcpServer::start();
    let dirs = TempDir::new("result-overwrite");
    let policies = PolicySet::from_yaml_str("tracker.summarize: {resultArtifact: page}").unwrap();
    let mut gw = gateway(&server, policies, &dirs, None);
    let failed = call("tracker", "summarize", json!({"toolError": "structured"}));
    assert!(gw.call(&failed).is_err());
    assert!(!dirs.path().join("artifacts/page.json").exists());
    assert!(!dirs.path().join("artifacts/page.json.meta.json").exists());
    for query in ["first", "second"] {
        let result = gw
            .call(&call("tracker", "summarize", json!({"query": query})))
            .unwrap();
        assert_eq!(read_result_artifact(&dirs), result.content_for_model);
    }
    let data = std::fs::read(dirs.path().join("artifacts/page.json")).unwrap();
    let meta = std::fs::read(dirs.path().join("artifacts/page.json.meta.json")).unwrap();
    assert!(gw.call(&failed).is_err());
    assert_eq!(
        std::fs::read(dirs.path().join("artifacts/page.json")).unwrap(),
        data
    );
    assert_eq!(
        std::fs::read(dirs.path().join("artifacts/page.json.meta.json")).unwrap(),
        meta
    );
    assert_eq!(read_result_artifact(&dirs)["summary"], "summary of second");
}

#[test]
fn result_artifact_failed_shape_is_not_complete() {
    let server = MockMcpServer::start();
    let dirs = TempDir::new("result-shape-failure");
    let policies = PolicySet::from_yaml_str(
        "tracker.get_issue: {resultArtifact: page, shape: {strip: [body], toFile: true}}",
    )
    .unwrap();
    let mut gw = gateway(&server, policies, &dirs, None);
    std::fs::create_dir_all(dirs.path().join("artifacts")).unwrap();
    // A file blocks the shape namespace directory, after raw data was written.
    std::fs::write(dirs.path().join("artifacts/tracker"), b"blocked").unwrap();
    assert!(gw
        .call(&call("tracker", "get_issue", json!({"id": 7})))
        .is_err());
    let meta: Value = serde_json::from_slice(
        &std::fs::read(dirs.path().join("artifacts/page.json.meta.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(meta["complete"], false);
}

#[test]
fn result_artifact_serve_from_hit_and_live_miss() {
    let server = MockMcpServer::start();
    let dirs = TempDir::new("result-serve");
    let mut policies = fixture_policies();
    policies
        .policies
        .get_mut("tracker.get_issue")
        .unwrap()
        .result_artifact = Some("page".into());
    let mut gw = gateway(&server, policies, &dirs, None);
    let c = call("tracker", "list_issues", json!({"query": "open"}));
    gw.call(&c).unwrap();
    wait_for_complete(
        &gw,
        "list_issues",
        &trollm_mcp::CacheStore::hash_args(&c.args, Some(&["query".into()]), Some("cursor")),
    );
    let hit = gw
        .call(&call("tracker", "get_issue", json!({"id": 1})))
        .unwrap();
    assert_eq!(hit.provenance, Provenance::ServedFromFile);
    assert_eq!(read_result_artifact(&dirs), hit.content_for_model);
    assert!(read_result_artifact(&dirs)["body"]
        .as_str()
        .unwrap()
        .contains("Full body"));
    let miss = gw
        .call(&call("tracker", "get_issue", json!({"id": 99})))
        .unwrap();
    assert_eq!(miss.provenance, Provenance::Live);
    assert_eq!(read_result_artifact(&dirs), miss.content_for_model);
    assert_eq!(server.count("call:get_issue"), 1);
}

fn wait_for_complete(gateway: &McpGateway, tool: &str, hash: &str) -> CacheMeta {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let meta_path = gateway
            .cache()
            .root()
            .join("tracker")
            .join(tool)
            .join(format!("{hash}.meta.json"));
        if meta_path.exists() {
            let meta: CacheMeta =
                serde_json::from_str(&std::fs::read_to_string(&meta_path).unwrap()).unwrap();
            if meta.complete {
                return meta;
            }
        }
        assert!(
            Instant::now() < deadline,
            "background fetch did not complete in time"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn collecting_sink() -> (Arc<Mutex<Vec<String>>>, EventSink) {
    let events: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let events_clone = events.clone();
    let sink: EventSink = Box::new(move |event| {
        events_clone.lock().unwrap().push(event.kind().to_string());
        Ok(())
    });
    (events, sink)
}

#[test]
fn policy_set_parses_canonical_flow_fixture() {
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../flows/examples/ticket-triage.flow.yaml");
    let yaml: serde_yaml::Value =
        serde_yaml::from_str(&std::fs::read_to_string(path).expect("read flow fixture"))
            .expect("parse flow yaml");
    let policies_value = yaml
        .get("mcp")
        .and_then(|m| m.get("policies"))
        .expect("flow has mcp.policies");
    let set = PolicySet::from_yaml_value(policies_value).expect("parse policies");

    assert_eq!(set.policies.len(), 5);
    let list = set
        .get("tracker", "list_issues")
        .expect("list_issues policy");
    assert_eq!(list.cache.as_ref().unwrap().ttl_s, 900);
    assert_eq!(list.cache.as_ref().unwrap().key, vec!["query".to_string()]);
    assert_eq!(list.paginate.as_ref().unwrap().param, "cursor");
    assert_eq!(list.shape.as_ref().unwrap().strip, vec!["body".to_string()]);
    assert!(list.shape.as_ref().unwrap().to_file);

    let get = set.get("tracker", "get_issue").expect("get_issue policy");
    let serve = get.serve_from.as_ref().unwrap();
    assert_eq!(serve.fetched_by, "tracker.list_issues");
    assert_eq!(serve.match_field, "id");

    let close = set
        .get("tracker", "close_issue")
        .expect("close_issue policy");
    assert_eq!(close.class, trollm_mcp::policy::ToolClass::Mutation);
    assert!(close.passthrough);
    assert_eq!(close.timeout_s, Some(10));
}

#[test]
fn list_tools_via_streamable_http() {
    let server = MockMcpServer::start();
    let dirs = TempDir::new("list");
    let gateway = gateway(&server, fixture_policies(), &dirs, None);
    let tools = gateway.list_tools().expect("list_tools");
    let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
    assert!(names.contains(&"list_issues"));
    assert!(names.contains(&"close_issue"));
    assert!(tools.iter().all(|t| t.server == "tracker"));
    assert_eq!(server.count("list"), 1);
    assert!(server.count("initialize") >= 1);
}

#[test]
fn read_cache_second_call_is_hit() {
    let server = MockMcpServer::start();
    let dirs = TempDir::new("cache");
    let policies = PolicySet::from_yaml_str(
        "tracker.summarize: { class: read, cache: { key: [query], ttlS: 900 } }",
    )
    .unwrap();
    let mut gw = gateway(&server, policies, &dirs, None);

    let args = json!({ "query": "open tickets" });
    let r1 = gw
        .call(&call("tracker", "summarize", args.clone()))
        .unwrap();
    assert_eq!(r1.provenance, Provenance::Live);
    assert_eq!(r1.content_for_model["summary"], "summary of open tickets");

    let r2 = gw.call(&call("tracker", "summarize", args)).unwrap();
    assert_eq!(r2.provenance, Provenance::Cache);
    assert_eq!(r2.content_for_model["summary"], "summary of open tickets");
    // Second identical call never touched the network.
    assert_eq!(server.count("call:summarize"), 1);

    // Different key args miss the cache.
    let r3 = gw
        .call(&call("tracker", "summarize", json!({ "query": "closed" })))
        .unwrap();
    assert_eq!(r3.provenance, Provenance::Live);
    assert_eq!(server.count("call:summarize"), 2);
}

#[test]
fn cache_ttl_expiry_refetches() {
    let server = MockMcpServer::start();
    let dirs = TempDir::new("ttl");
    let policies = PolicySet::from_yaml_str(
        "tracker.summarize: { class: read, cache: { key: [query], ttlS: 0 } }",
    )
    .unwrap();
    let mut gw = gateway(&server, policies, &dirs, None);
    let args = json!({ "query": "anything" });
    let r1 = gw
        .call(&call("tracker", "summarize", args.clone()))
        .unwrap();
    assert_eq!(r1.provenance, Provenance::Live);
    // ttlS: 0 -> immediately stale.
    let r2 = gw.call(&call("tracker", "summarize", args)).unwrap();
    assert_eq!(r2.provenance, Provenance::Live);
    assert_eq!(server.count("call:summarize"), 2);
}

#[test]
fn paginate_first_page_live_then_pages_from_file() {
    let server = MockMcpServer::start();
    let dirs = TempDir::new("paginate");
    let (events, sink) = collecting_sink();
    let mut gw = gateway(&server, fixture_policies(), &dirs, Some(sink));

    // First call: page 1 comes back from the live call immediately.
    let r1 = gw
        .call(&call("tracker", "list_issues", json!({ "query": "open" })))
        .unwrap();
    assert_eq!(r1.provenance, Provenance::Live);
    let issues = r1.content_for_model["issues"].as_array().unwrap();
    assert_eq!(issues.len(), 2);
    assert_eq!(issues[0]["id"], 1);
    assert_eq!(r1.content_for_model["cursor"], 2);

    // Background task completes the cache file.
    let hash = trollm_mcp::CacheStore::hash_args(
        &json!({ "query": "open" }),
        Some(&["query".to_string()]),
        Some("cursor"),
    );
    let meta = wait_for_complete(&gw, "list_issues", &hash);
    assert!(meta.complete);
    assert_eq!(meta.pages, 3);
    assert_eq!(meta.total_items, Some(6));
    assert_eq!(meta.ttl_s, 900);
    // 1 synchronous page-1 fetch + 2 background pages.
    assert_eq!(server.count("call:list_issues"), 3);

    // Page 2 is sliced from the local file — no network.
    let r2 = gw
        .call(&call(
            "tracker",
            "list_issues",
            json!({ "query": "open", "cursor": 2 }),
        ))
        .unwrap();
    assert_eq!(r2.provenance, Provenance::Cache);
    let page2 = r2.content_for_model["issues"].as_array().unwrap();
    assert_eq!(page2[0]["id"], 3);
    assert_eq!(page2[1]["id"], 4);
    assert_eq!(r2.content_for_model["cursor"], 3);
    let r3 = gw
        .call(&call(
            "tracker",
            "list_issues",
            json!({ "query": "open", "cursor": 3 }),
        ))
        .unwrap();
    assert_eq!(r3.provenance, Provenance::Cache);
    assert_eq!(r3.content_for_model["cursor"], Value::Null);
    assert_eq!(server.count("call:list_issues"), 3);

    let kinds = events.lock().unwrap().clone();
    assert_eq!(&kinds[..3], &["cache_miss", "live_call", "fetch_started"]);
    assert!(kinds.contains(&"fetch_progress".to_string()));
    assert!(kinds.contains(&"fetch_complete".to_string()));
    assert_eq!(kinds.iter().filter(|k| *k == "cache_hit").count(), 2);
    let fetch_complete_pos = kinds.iter().position(|k| k == "fetch_complete").unwrap();
    let first_hit_pos = kinds.iter().position(|k| k == "cache_hit").unwrap();
    assert!(fetch_complete_pos < first_hit_pos);
}

#[test]
fn serve_from_hit_and_fetch_live_miss() {
    let server = MockMcpServer::start();
    let dirs = TempDir::new("servefrom");
    let mut gw = gateway(&server, fixture_policies(), &dirs, None);

    // Prime the source tool's cache to complete.
    gw.call(&call("tracker", "list_issues", json!({ "query": "open" })))
        .unwrap();
    let hash = trollm_mcp::CacheStore::hash_args(
        &json!({ "query": "open" }),
        Some(&["query".to_string()]),
        Some("cursor"),
    );
    wait_for_complete(&gw, "list_issues", &hash);

    // Hit: served from the cached file, no network for get_issue.
    let hit = gw
        .call(&call("tracker", "get_issue", json!({ "id": 5 })))
        .unwrap();
    assert_eq!(hit.provenance, Provenance::ServedFromFile);
    assert_eq!(hit.content_for_model["id"], 5);
    assert_eq!(hit.content_for_model["title"], "Issue 5");
    assert_eq!(server.count("call:get_issue"), 0);

    // Miss: onMiss = fetch_live falls back to the network.
    let miss = gw
        .call(&call("tracker", "get_issue", json!({ "id": 999 })))
        .unwrap();
    assert_eq!(miss.provenance, Provenance::Live);
    assert_eq!(miss.content_for_model["id"], 999);
    assert_eq!(server.count("call:get_issue"), 1);
}

#[test]
fn serve_from_on_miss_fail_errors() {
    let server = MockMcpServer::start();
    let dirs = TempDir::new("servefrom-fail");
    let policies = PolicySet::from_yaml_str(
        r#"
tracker.list_issues:
  class: read
  cache: { key: [query], ttlS: 900 }
  paginate: { param: cursor, deliver: first_page, backgroundFetch: all }
tracker.get_issue:
  class: read
  serveFrom: { fetchedBy: tracker.list_issues, match: id }
  onMiss: fail
"#,
    )
    .unwrap();
    let mut gw = gateway(&server, policies, &dirs, None);
    // Cold cache + onMiss: fail -> error, no live call.
    let err = gw.call(&call("tracker", "get_issue", json!({ "id": 1 })));
    assert!(err.is_err());
    assert_eq!(server.count("call:get_issue"), 0);
}

#[test]
fn shape_strips_field_to_artifact_file() {
    let server = MockMcpServer::start();
    let dirs = TempDir::new("shape");
    let (events, sink) = collecting_sink();
    let mut gw = gateway(&server, fixture_policies(), &dirs, Some(sink));

    let r = gw
        .call(&call("tracker", "list_issues", json!({ "query": "open" })))
        .unwrap();
    let issues = r.content_for_model["issues"].as_array().unwrap();
    assert_eq!(issues.len(), 2);
    assert_eq!(r.artifacts.len(), 3);
    for issue in issues {
        let body = &issue["body"];
        let path = body["path"].as_str().expect("stripped field has path");
        assert!(body["bytes"].as_u64().unwrap() > 0);
        assert!(body["preview"].as_str().unwrap().contains("Full body text"));
        // Artifact file holds the stripped content.
        let on_disk: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        assert!(on_disk.as_str().unwrap().contains("Full body text"));
        assert_eq!(
            body["bytes"].as_u64().unwrap() as usize,
            std::fs::metadata(path).unwrap().len() as usize
        );
    }
    let kinds = events.lock().unwrap().clone();
    assert!(kinds.iter().filter(|k| *k == "shaped_to_file").count() == 2);
}

#[test]
fn mutation_passthrough_never_touches_cache() {
    let server = MockMcpServer::start();
    let dirs = TempDir::new("mutation");
    let (events, sink) = collecting_sink();
    let mut gw = gateway(&server, fixture_policies(), &dirs, Some(sink));

    let r = gw
        .call(&call("tracker", "close_issue", json!({ "id": 3 })))
        .unwrap();
    assert_eq!(r.provenance, Provenance::Live);
    assert_eq!(r.content_for_model["ok"], true);
    assert_eq!(server.count("call:close_issue"), 1);

    // No cache entry was written for the mutation tool.
    assert!(!dirs.path().join("cache/tracker/close_issue").exists());

    // Intent event fires before the live call result event.
    let kinds = events.lock().unwrap().clone();
    assert_eq!(kinds, vec!["mutation_passthrough", "live_call"]);

    // Same mutation again: still goes to the network (never cached).
    gw.call(&call("tracker", "close_issue", json!({ "id": 3 })))
        .unwrap();
    assert_eq!(server.count("call:close_issue"), 2);
}

fn mutation_result_policies(shape: bool) -> PolicySet {
    let mut policy = json!({
        "class":"mutation", "passthrough":true,
        "retry":{"attempts":1,"backoffS":0},
        "cache":{"key":["id"],"ttlS":900},
        "resultArtifact":"page",
        "resultValidation":{"maxBytes":256,"schema":{
            "type":"object","required":["result","tabId"],
            "properties":{"result":{"const":"opened"},"tabId":{"type":"integer","minimum":1}}
        }}
    });
    if shape {
        policy["shape"] = json!({"strip":["body"],"toFile":true});
    }
    PolicySet::from_yaml_str(&json!({"tracker.close_issue":policy}).to_string()).unwrap()
}

#[test]
fn mutation_result_artifact_preserves_valid_payload_before_shape_without_cache_or_retry() {
    let server = MockMcpServer::start();
    let dirs = TempDir::new("mutation-result");
    let payload = json!({"result":"opened","tabId":178,"body":"exact unshaped result"});
    server
        .tool_payloads
        .lock()
        .unwrap()
        .insert("close_issue".into(), payload.clone());
    let (events, sink) = collecting_sink();
    let mut gw = gateway(&server, mutation_result_policies(true), &dirs, Some(sink));
    let c = call("tracker", "close_issue", json!({"id":3}));
    let result = gw.call(&c).unwrap();
    assert_eq!(result.provenance, Provenance::Live);
    assert_eq!(result.content_for_model["tabId"], 178);
    assert!(result.content_for_model["body"].is_object());
    assert_eq!(read_result_artifact(&dirs), payload);
    assert!(result
        .artifacts
        .contains(&dirs.path().join("artifacts/page.json")));
    assert_eq!(server.count("call:close_issue"), 1);
    assert_eq!(
        events.lock().unwrap().as_slice(),
        ["mutation_passthrough", "live_call", "shaped_to_file"]
    );
    // A second explicit caller request is another effect, never a cache hit.
    let again = gw.call(&c).unwrap();
    assert_eq!(again.provenance, Provenance::Live);
    assert_eq!(server.count("call:close_issue"), 2);
    let hash = trollm_mcp::CacheStore::hash_args(&c.args, Some(&["id".into()]), None);
    assert!(gw
        .cache()
        .fresh_entry("tracker", "close_issue", &hash)
        .unwrap()
        .is_none());
}

#[test]
fn malformed_or_oversized_mutation_results_are_not_published_or_retried() {
    for payload in [
        json!({"result":"opened","tabId":"private-invalid-id"}),
        json!({"result":"opened","tabId":178,"body":"x".repeat(300)}),
    ] {
        let server = MockMcpServer::start();
        let dirs = TempDir::new("mutation-result-rejected");
        server
            .tool_payloads
            .lock()
            .unwrap()
            .insert("close_issue".into(), payload.clone());
        let (events, sink) = collecting_sink();
        let mut gw = gateway(&server, mutation_result_policies(true), &dirs, Some(sink));
        let error = format!(
            "{:#}",
            gw.call(&call("tracker", "close_issue", json!({"id":3})))
                .unwrap_err()
        );
        assert!(error.contains("tool result"), "{error}");
        assert!(!error.contains("private-invalid-id"), "{error}");
        if payload.get("body").is_some() {
            assert!(error.contains("serialized size limit"), "{error}");
        }
        assert_eq!(server.count("call:close_issue"), 1);
        assert_eq!(
            events.lock().unwrap().as_slice(),
            ["mutation_passthrough", "live_call"]
        );
        assert!(!dirs.path().join("artifacts/page.json").exists());
        assert!(!dirs.path().join("artifacts/tracker").exists());
        let meta: Value = serde_json::from_slice(
            &std::fs::read(dirs.path().join("artifacts/page.json.meta.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(meta["complete"], false);
    }
}

#[test]
fn invalid_mutation_replacement_marks_prior_artifact_incomplete_without_replay() {
    let server = MockMcpServer::start();
    let dirs = TempDir::new("mutation-result-replacement");
    let good = json!({"result":"opened","tabId":178});
    server
        .tool_payloads
        .lock()
        .unwrap()
        .insert("close_issue".into(), good.clone());
    let mut gw = gateway(&server, mutation_result_policies(false), &dirs, None);
    let c = call("tracker", "close_issue", json!({"id":3}));
    gw.call(&c).unwrap();
    assert_eq!(read_result_artifact(&dirs), good);
    server
        .tool_payloads
        .lock()
        .unwrap()
        .insert("close_issue".into(), json!({"result":"opened"}));
    assert!(gw.call(&c).is_err());
    assert_eq!(server.count("call:close_issue"), 2);
    let path = dirs.path().join("artifacts/page.json");
    let meta: Value =
        serde_json::from_slice(&std::fs::read(path.with_file_name("page.json.meta.json")).unwrap())
            .unwrap();
    assert_eq!(meta["complete"], false);
    let prior: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    assert_eq!(prior, good);
}

#[test]
fn mutation_publication_or_shape_failure_after_effect_does_not_replay() {
    for block_shape in [false, true] {
        let server = MockMcpServer::start();
        let dirs = TempDir::new("mutation-result-write-failure");
        server.tool_payloads.lock().unwrap().insert(
            "close_issue".into(),
            json!({"result":"opened","tabId":178,"body":"exact result"}),
        );
        let (events, sink) = collecting_sink();
        let mut gw = gateway(&server, mutation_result_policies(true), &dirs, Some(sink));
        let artifact_dir = dirs.path().join("artifacts");
        std::fs::create_dir_all(&artifact_dir).unwrap();
        if block_shape {
            std::fs::write(artifact_dir.join("tracker"), b"blocked").unwrap();
        } else {
            std::fs::create_dir(artifact_dir.join("page.json")).unwrap();
        }
        assert!(gw
            .call(&call("tracker", "close_issue", json!({"id":3})))
            .is_err());
        assert_eq!(server.count("call:close_issue"), 1);
        assert_eq!(
            events.lock().unwrap().as_slice(),
            ["mutation_passthrough", "live_call"]
        );
        let meta: Value = serde_json::from_slice(
            &std::fs::read(artifact_dir.join("page.json.meta.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(meta["complete"], false);
    }
}

#[test]
fn event_sink_receives_cache_miss_then_hit_in_order() {
    let server = MockMcpServer::start();
    let dirs = TempDir::new("events");
    let policies = PolicySet::from_yaml_str(
        "tracker.summarize: { class: read, cache: { key: [query], ttlS: 900 } }",
    )
    .unwrap();
    let (events, sink) = collecting_sink();
    let mut gw = gateway(&server, policies, &dirs, Some(sink));

    let args = json!({ "query": "q" });
    gw.call(&call("tracker", "summarize", args.clone()))
        .unwrap();
    gw.call(&call("tracker", "summarize", args)).unwrap();

    let kinds = events.lock().unwrap().clone();
    assert_eq!(kinds, vec!["cache_miss", "live_call", "cache_hit"]);
}

#[test]
fn worklist_is_complete_before_cold_return_and_republished_on_warm_hit() {
    let server = MockMcpServer::start();
    let dirs = TempDir::new("worklist");
    let mut gw = gateway(&server, fixture_policies(), &dirs, None);
    let c = call(
        "tracker",
        "list_issues",
        json!({"query": "open", "cursor": 2}),
    );
    let cold = gw.call(&c).unwrap();
    let path = dirs
        .path()
        .join("artifacts/tracker.list_issues.latest.jsonl");
    assert!(cold.artifacts.contains(&path));
    let body = std::fs::read_to_string(&path).unwrap();
    assert_eq!(body.lines().count(), 6);
    assert_eq!(
        serde_json::from_str::<Value>(body.lines().last().unwrap()).unwrap()["id"],
        6
    );
    let meta_path = path.with_file_name("tracker.list_issues.latest.jsonl.meta.json");
    let meta: Value = serde_json::from_slice(&std::fs::read(&meta_path).unwrap()).unwrap();
    assert_eq!(meta["complete"], true);
    assert_eq!(meta["totalItems"], 6);
    std::fs::remove_file(&path).unwrap();
    std::fs::remove_file(&meta_path).unwrap();
    let warm = gw.call(&c).unwrap();
    assert_eq!(warm.provenance, Provenance::Cache);
    assert!(warm.artifacts.contains(&path));
    assert_eq!(std::fs::read_to_string(path).unwrap(), body);
    assert_eq!(server.count("call:list_issues"), 3);
}

#[test]
fn tool_error_envelopes_are_errors_and_never_cached() {
    let server = MockMcpServer::start();
    let dirs = TempDir::new("errors");
    let policies = PolicySet::from_yaml_str(
        "tracker.summarize: {class: read, cache: {key: [query], ttlS: 900}}",
    )
    .unwrap();
    let mut gw = gateway(&server, policies, &dirs, None);
    for kind in ["structured", "json_text", "text"] {
        let c = call(
            "tracker",
            "summarize",
            json!({"query": kind, "toolError":kind}),
        );
        for _ in 0..2 {
            assert!(gw.call(&c).unwrap_err().to_string().contains("isError"));
        }
    }
    assert_eq!(server.count("call:summarize"), 6);
}

#[test]
fn stateless_session_is_initialized_once() {
    let server = MockMcpServer::start_with_session(false);
    let dirs = TempDir::new("stateless");
    let gw = gateway(&server, PolicySet::default(), &dirs, None);
    gw.list_tools().unwrap();
    gw.list_tools().unwrap();
    assert_eq!(server.count("initialize"), 1);
}

#[test]
fn production_rejects_http_before_credential_lookup() {
    let looked_up = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let captured = looked_up.clone();
    let dirs = TempDir::new("https");
    let server = ServerConfig::new("tracker", "http://127.0.0.1:1/mcp").with_token_getter(
        Arc::new(move || {
            captured.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Some("secret".into())
        }),
    );
    assert!(McpGateway::new(
        vec![server],
        PolicySet::default(),
        dirs.path().join("cache"),
        dirs.path().join("artifacts"),
        None
    )
    .is_err());
    assert_eq!(looked_up.load(std::sync::atomic::Ordering::SeqCst), 0);
}

#[test]
fn redirect_is_rejected_without_contacting_target() {
    let server = MockMcpServer::start();
    let target = MockMcpServer::start();
    let dirs = TempDir::new("redirect");
    let mut gw = gateway(&server, PolicySet::default(), &dirs, None);
    assert!(gw
        .call(&call("tracker", "redirect", json!({"target":target.url})))
        .is_err());
    assert_eq!(target.count("initialize"), 0);
    assert_eq!(target.count("call:redirect"), 0);
}

#[test]
fn ambiguous_mutation_failure_is_not_retried() {
    let server = MockMcpServer::start();
    let dirs = TempDir::new("retry");
    let policies = PolicySet::from_yaml_str(
        "tracker.drop_mutation: {class: mutation, retry: {attempts: 1, backoffS: 0}}",
    )
    .unwrap();
    let mut gw = gateway(&server, policies, &dirs, None);
    assert!(gw
        .call(&call("tracker", "drop_mutation", json!({})))
        .is_err());
    assert_eq!(server.count("call:drop_mutation"), 1);
}

#[test]
fn cyclic_pagination_fails_without_publishing_a_complete_artifact() {
    let server = MockMcpServer::start();
    let dirs = TempDir::new("cycle");
    let mut gw = gateway(&server, fixture_policies(), &dirs, None);
    assert!(gw
        .call(&call("tracker", "list_issues", json!({"query":"cycle"})))
        .unwrap_err()
        .to_string()
        .contains("cursor repeated"));
    assert_eq!(server.count("call:list_issues"), 2);
    assert!(!dirs
        .path()
        .join("artifacts/tracker.list_issues.latest.jsonl")
        .exists());
}

#[test]
fn gateways_never_share_cache_generations_or_endpoint_data() {
    let a = MockMcpServer::start();
    let b = MockMcpServer::start();
    let dirs = TempDir::new("isolation");
    let policies = PolicySet::from_yaml_str(
        "tracker.summarize: {class: read, cache: {key: [query], ttlS: 900}}",
    )
    .unwrap();
    let mut first = gateway(&a, policies.clone(), &dirs, None);
    let mut second = gateway(&b, policies, &dirs, None);
    let c = call("tracker", "summarize", json!({"query":"open"}));
    first.call(&c).unwrap();
    assert_ne!(first.cache().root(), second.cache().root());
    assert_eq!(second.call(&c).unwrap().provenance, Provenance::Live);
    assert_eq!(first.call(&c).unwrap().provenance, Provenance::Cache);
    assert_eq!(a.count("call:summarize"), 1);
    assert_eq!(b.count("call:summarize"), 1);
}

#[test]
fn credential_is_bound_once_after_validation_and_requires_gateway_rebuild_to_rotate() {
    let server = MockMcpServer::start();
    let dirs = TempDir::new("credentials");
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let captured = calls.clone();
    let config = ServerConfig::for_local_test("tracker", &server.url).with_token_getter(Arc::new(
        move || {
            captured.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Some("fixture-token".to_string())
        },
    ));
    let mut gw = McpGateway::new(
        vec![config],
        PolicySet::default(),
        dirs.path().join("cache"),
        dirs.path().join("artifacts"),
        None,
    )
    .unwrap();
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    gw.list_tools().unwrap();
    gw.call(&call("tracker", "summarize", json!({"query":"open"})))
        .unwrap();
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
}

#[test]
fn concurrent_gateways_own_independent_complete_generations() {
    let server = MockMcpServer::start();
    let dirs = TempDir::new("concurrent");
    let mut first = gateway(&server, fixture_policies(), &dirs, None);
    let second_dirs = TempDir::new("concurrent-second");
    let mut second = McpGateway::new(
        vec![ServerConfig::for_local_test("tracker", &server.url)],
        fixture_policies(),
        dirs.path().join("cache"),
        second_dirs.path().join("artifacts"),
        None,
    )
    .unwrap();
    let first_root = first.cache().root().to_owned();
    let second_root = second.cache().root().to_owned();
    assert_ne!(first_root, second_root);
    // Different run artifact directories, matching production wiring.
    let barrier = Arc::new(std::sync::Barrier::new(2));
    let b = barrier.clone();
    let h = std::thread::spawn(move || {
        b.wait();
        first
            .call(&call("tracker", "list_issues", json!({"query":"shared"})))
            .unwrap();
        first
    });
    barrier.wait();
    second
        .call(&call("tracker", "list_issues", json!({"query":"shared"})))
        .unwrap();
    let first = h.join().unwrap();
    for (gw, query) in [(&first, "shared"), (&second, "shared")] {
        let hash = trollm_mcp::CacheStore::hash_args(
            &json!({"query":query}),
            Some(&["query".to_string()]),
            Some("cursor"),
        );
        let entry = gw
            .cache()
            .fresh_entry("tracker", "list_issues", &hash)
            .unwrap()
            .unwrap();
        assert_eq!(entry.meta.pages, 3);
        assert_eq!(
            gw.cache()
                .read_items("tracker", "list_issues", &hash)
                .unwrap()
                .len(),
            6
        );
    }
}

#[test]
fn pagination_page_bound_stops_unique_nonterminal_cursors() {
    let server = MockMcpServer::start();
    let dirs = TempDir::new("page-limit");
    let mut gw = gateway(&server, fixture_policies(), &dirs, None);
    assert!(gw
        .call(&call("tracker", "list_issues", json!({"query":"endless"})))
        .unwrap_err()
        .to_string()
        .contains("page or time limit"));
    assert_eq!(server.count("call:list_issues"), 256);
    assert!(!dirs
        .path()
        .join("artifacts/tracker.list_issues.latest.jsonl")
        .exists());
}

#[test]
fn distinct_paginated_tools_with_equal_arguments_each_complete() {
    let server = MockMcpServer::start();
    let dirs = TempDir::new("tool-identity");
    let policies = PolicySet::from_yaml_str("tracker.list_issues: {class: read, cache: {key: [query], ttlS: 900}, paginate: {param: cursor, deliver: first_page, backgroundFetch: all}}\ntracker.list_issues_alt: {class: read, cache: {key: [query], ttlS: 900}, paginate: {param: cursor, deliver: first_page, backgroundFetch: all}}").unwrap();
    let mut gw = gateway(&server, policies, &dirs, None);
    for tool in ["list_issues", "list_issues_alt"] {
        let result = gw
            .call(&call("tracker", tool, json!({"query":"open"})))
            .unwrap();
        let path = dirs
            .path()
            .join(format!("artifacts/tracker.{tool}.latest.jsonl"));
        assert!(result.artifacts.contains(&path));
        assert_eq!(std::fs::read_to_string(path).unwrap().lines().count(), 6);
        assert_eq!(server.count(&format!("call:{tool}")), 3);
    }
}

#[test]
fn zero_ttl_pagination_publishes_completed_worklist_and_refreshes_next_call() {
    let server = MockMcpServer::start();
    let dirs = TempDir::new("zero-ttl");
    let policies = PolicySet::from_yaml_str("tracker.list_issues: {class: read, cache: {key: [query], ttlS: 0}, paginate: {param: cursor, deliver: first_page, backgroundFetch: all}}").unwrap();
    let mut gw = gateway(&server, policies, &dirs, None);
    for _ in 0..2 {
        let result = gw
            .call(&call("tracker", "list_issues", json!({"query":"open"})))
            .unwrap();
        assert_eq!(result.provenance, Provenance::Live);
        assert_eq!(
            std::fs::read_to_string(&result.artifacts[0])
                .unwrap()
                .lines()
                .count(),
            6
        );
    }
    assert_eq!(server.count("call:list_issues"), 6);
}

#[test]
fn grant_credentials_refresh_headers_and_invalidation_blocks_cached_and_live_returns() {
    let server = MockMcpServer::start();
    let dirs = TempDir::new("grant-refresh");
    let token = Arc::new(Mutex::new(Some("grant-a-access-1".to_string())));
    let captured = token.clone();
    let config = ServerConfig::for_local_test("tracker", &server.url)
        .with_grant_token_getter(Arc::new(move || captured.lock().unwrap().clone()));
    let policies =
        PolicySet::from_yaml_str("tracker.summarize: {cache: {key: [query], ttlS: 900}}").unwrap();
    let mut gw = McpGateway::new(
        vec![config],
        policies,
        dirs.path().join("cache"),
        dirs.path().join("artifacts"),
        None,
    )
    .unwrap();
    let first = call("tracker", "summarize", json!({"query": "first"}));
    gw.call(&first).unwrap();
    assert!(server
        .request_auth
        .lock()
        .unwrap()
        .iter()
        .all(|(_, auth)| auth.as_deref() == Some("Bearer grant-a-access-1")));
    *token.lock().unwrap() = Some("grant-a-access-2".into());
    gw.call(&call("tracker", "summarize", json!({"query": "second"})))
        .unwrap();
    assert_eq!(
        server
            .request_auth
            .lock()
            .unwrap()
            .last()
            .unwrap()
            .1
            .as_deref(),
        Some("Bearer grant-a-access-2")
    );
    let requests = server.request_auth.lock().unwrap().len();
    assert_eq!(gw.call(&first).unwrap().provenance, Provenance::Cache);
    assert_eq!(server.request_auth.lock().unwrap().len(), requests);
    for invalid in [None, Some("  \t\n".to_string())] {
        *token.lock().unwrap() = invalid;
        for result in [
            gw.list_tools().map(|_| ()),
            gw.call(&first).map(|_| ()),
            gw.call(&call("tracker", "summarize", json!({"query": "uncached"})))
                .map(|_| ()),
        ] {
            let error = format!("{:#}", result.unwrap_err());
            assert!(error.contains("reconnect this connection"));
            assert!(!error.contains("grant-a-access"));
        }
        assert_eq!(server.request_auth.lock().unwrap().len(), requests);
    }
}

#[test]
fn invalid_grant_never_initializes_or_falls_back_to_anonymous() {
    let server = MockMcpServer::start();
    let dirs = TempDir::new("grant-invalid");
    let config = ServerConfig::for_local_test("tracker", &server.url)
        .with_grant_token_getter(Arc::new(|| None));
    let mut gw = McpGateway::new(
        vec![config],
        PolicySet::default(),
        dirs.path().join("cache"),
        dirs.path().join("artifacts"),
        None,
    )
    .unwrap();
    assert!(gw.list_tools().is_err());
    assert!(gw
        .call(&call("tracker", "summarize", json!({"query": "test"})))
        .is_err());
    assert!(server.request_auth.lock().unwrap().is_empty());
}

#[test]
fn cross_server_serve_from_requires_source_and_target_grants() {
    let source = MockMcpServer::start();
    let target = MockMcpServer::start();
    let dirs = TempDir::new("grant-serve");
    let source_token = Arc::new(Mutex::new(Some("source-grant".to_string())));
    let target_token = Arc::new(Mutex::new(Some("target-grant".to_string())));
    let a = source_token.clone();
    let b = target_token.clone();
    let policies = PolicySet::from_yaml_str(
        r#"
tracker.list_issues:
  cache: {key: [query], ttlS: 900}
  paginate: {param: cursor, deliver: first_page, backgroundFetch: all}
reader.get_issue:
  serveFrom: {fetchedBy: tracker.list_issues, match: id}
  onMiss: fail
"#,
    )
    .unwrap();
    let mut gw = McpGateway::new(
        vec![
            ServerConfig::for_local_test("tracker", &source.url)
                .with_grant_token_getter(Arc::new(move || a.lock().unwrap().clone())),
            ServerConfig::for_local_test("reader", &target.url)
                .with_grant_token_getter(Arc::new(move || b.lock().unwrap().clone())),
        ],
        policies,
        dirs.path().join("cache"),
        dirs.path().join("artifacts"),
        None,
    )
    .unwrap();
    gw.call(&call("tracker", "list_issues", json!({"query": "open"})))
        .unwrap();
    let c = call("reader", "get_issue", json!({"id": 1}));
    assert_eq!(gw.call(&c).unwrap().provenance, Provenance::ServedFromFile);
    let source_requests = source.request_auth.lock().unwrap().len();
    let target_requests = target.request_auth.lock().unwrap().len();
    *source_token.lock().unwrap() = None;
    assert!(format!("{:#}", gw.call(&c).unwrap_err()).contains("reconnect"));
    assert!(gw
        .call(&call("tracker", "list_issues", json!({"query": "open"})))
        .is_err());
    *source_token.lock().unwrap() = Some("source-grant-refreshed".into());
    *target_token.lock().unwrap() = None;
    assert!(format!("{:#}", gw.call(&c).unwrap_err()).contains("reconnect"));
    assert_eq!(source.request_auth.lock().unwrap().len(), source_requests);
    assert_eq!(target.request_auth.lock().unwrap().len(), target_requests);
}

#[test]
fn all_endpoint_urls_are_validated_before_any_grant_or_legacy_getter() {
    let dirs = TempDir::new("grant-url-validation");
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let a = calls.clone();
    let b = calls.clone();
    let servers = vec![
        ServerConfig::new("grant", "https://example.com/mcp").with_grant_token_getter(Arc::new(
            move || {
                a.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Some("unused".into())
            },
        )),
        ServerConfig::new("legacy", "https://example.com/mcp").with_token_getter(Arc::new(
            move || {
                b.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Some("unused".into())
            },
        )),
        ServerConfig::new("invalid", "http://example.com/mcp"),
    ];
    assert!(McpGateway::new(
        servers,
        PolicySet::default(),
        dirs.path().join("cache"),
        dirs.path().join("artifacts"),
        None
    )
    .is_err());
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 0);
}

#[test]
fn grant_mutation_http_401_is_not_replayed_or_retried() {
    let server = MockMcpServer::start();
    let dirs = TempDir::new("grant-401");
    let config = ServerConfig::for_local_test("tracker", &server.url)
        .with_grant_token_getter(Arc::new(|| Some("grant-access".into())));
    let policies = PolicySet::from_yaml_str(
        "tracker.close_issue: {class: mutation, retry: {attempts: 1, backoffS: 0}}",
    )
    .unwrap();
    let mut gw = McpGateway::new(
        vec![config],
        policies,
        dirs.path().join("cache"),
        dirs.path().join("artifacts"),
        None,
    )
    .unwrap();
    let error = format!(
        "{:#}",
        gw.call(&call(
            "tracker",
            "close_issue",
            json!({"id": 1, "httpStatus": 401})
        ))
        .unwrap_err()
    );
    assert!(error.contains("HTTP 401"));
    assert!(!error.contains("grant-access"));
    assert_eq!(server.count("call:close_issue"), 1);
}

fn page_result_schema() -> Value {
    json!({"type":"object","required":["content","stats"],"properties":{
        "content":{"type":"string","minLength":1},
        "stats":{"type":"object","required":["truncated","contentNodes"],"properties":{
            "truncated":{"const":false},"contentNodes":{"type":"integer","minimum":1}}}}})
}

fn validation_policies(extra: Value, max_bytes: usize, schema: Value) -> PolicySet {
    let mut policy = extra;
    policy["resultValidation"] = json!({"maxBytes":max_bytes,"schema":schema});
    PolicySet::from_yaml_str(&json!({"tracker.get_issue":policy}).to_string()).unwrap()
}

#[test]
fn result_validation_policy_is_bounded_non_paginated_and_strict_inside_its_object() {
    for invalid in [
        json!({"maxBytes":0,"schema":true}),
        json!({"maxBytes":262145,"schema":true}),
        json!({"maxBytes":64}),
        json!({"schema":true}),
        json!({"maxBytes":64,"schema":true,"unknown":true}),
        json!({"maxBytes":-1,"schema":true}),
        json!({"maxBytes":"64","schema":true}),
    ] {
        assert!(PolicySet::from_yaml_str(
            &json!({"tracker.get_issue":{"resultValidation":invalid}}).to_string()
        )
        .is_err());
    }
    for class in ["read", "mutation"] {
        let mut policy = json!({"class":class,"paginate":{"param":"cursor","deliver":"first_page","backgroundFetch":"all"}});
        policy["resultValidation"] = json!({"maxBytes":64,"schema":true});
        assert!(
            PolicySet::from_yaml_str(&json!({"tracker.get_issue":policy}).to_string()).is_err()
        );
    }
    assert!(PolicySet::from_yaml_str(
        r#"tracker.close_issue: {class: mutation, resultValidation: {maxBytes: 64, schema: true}}"#
    )
    .is_ok());
    // The enclosing policy is also a closed authoring object.
    assert!(PolicySet::from_yaml_str(r#"tracker.get_issue: {existingUnknown: true, resultValidation: {maxBytes: 262144, schema: true}}"#).is_err());
}

#[test]
fn result_validation_schemas_compile_offline_before_credentials_are_consulted() {
    for schema in [
        json!({"type":"not-a-schema-type"}),
        json!({"$ref":"https://secret-schema.invalid/private"}),
        json!({"$ref":"file:///private/secret-schema.json"}),
    ] {
        let policies = validation_policies(json!({}), 1024, schema);
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let count = calls.clone();
        let dirs = TempDir::new("invalid-result-schema");
        let result = McpGateway::new(
            vec![
                ServerConfig::new("tracker", "https://example.org/mcp").with_token_getter(
                    Arc::new(move || {
                        count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        Some("fixture-secret".into())
                    }),
                ),
            ],
            policies,
            dirs.path().join("cache"),
            dirs.path().join("artifacts"),
            None,
        );
        let error = match result {
            Ok(_) => panic!("invalid result schema accepted"),
            Err(error) => format!("{error:#}"),
        };
        assert!(error.contains("tool result schema"), "{error}");
        assert!(!error.contains("secret-schema"), "{error}");
        assert!(!error.contains("fixture-secret"), "{error}");
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 0);
    }
}

#[test]
fn result_validation_accepts_exact_live_and_cached_payload_before_shaping() {
    let server = MockMcpServer::start();
    let dirs = TempDir::new("validated-result-live");
    let policies = validation_policies(
        json!({"resultArtifact":"page","cache":{"key":["id"],"ttlS":900},
        "shape":{"strip":["body"],"toFile":true}}),
        1024,
        json!({"type":"object","required":["body"],"properties":{"body":{"const":"live body"}}}),
    );
    let mut gw = gateway(&server, policies, &dirs, None);
    let c = call("tracker", "get_issue", json!({"id":7}));
    let expected = json!({"id":7,"title":"Issue 7","body":"live body"});
    for provenance in [Provenance::Live, Provenance::Cache] {
        let result = gw.call(&c).unwrap();
        assert_eq!(result.provenance, provenance);
        assert!(result.content_for_model["body"].is_object());
        assert_eq!(read_result_artifact(&dirs), expected);
    }
    assert_eq!(server.count("call:get_issue"), 1);
}

#[test]
fn result_validation_rejects_cached_partial_empty_and_oversize_without_stale_artifact() {
    let server = MockMcpServer::start();
    let dirs = TempDir::new("validated-result-cache");
    let policies = validation_policies(
        json!({"resultArtifact":"page","cache":{"key":["id"],"ttlS":900}}),
        256,
        page_result_schema(),
    );
    let mut gw = gateway(&server, policies, &dirs, None);
    let c = call("tracker", "get_issue", json!({"id":7}));
    let hash = trollm_mcp::CacheStore::hash_args(&c.args, Some(&["id".into()]), None);
    let good =
        json!({"content":"Exact accepted page","stats":{"truncated":false,"contentNodes":1}});
    let bad = [
        json!({"content":"private-result-must-not-appear","stats":{"truncated":true,"contentNodes":1}}),
        json!({"content":"private-result-must-not-appear","stats":{"truncated":false,"contentNodes":0}}),
        json!({"content":"","stats":{"truncated":false,"contentNodes":1}}),
        // It is also schema-invalid: size rejection must happen first.
        json!({"private-result-must-not-appear":"x".repeat(300)}),
    ];
    for (index, payload) in bad.into_iter().enumerate() {
        gw.cache()
            .write_single("tracker", "get_issue", &hash, &good, 900, &c.args)
            .unwrap();
        assert_eq!(gw.call(&c).unwrap().provenance, Provenance::Cache);
        assert_eq!(read_result_artifact(&dirs), good);
        let path = dirs.path().join("artifacts/page.json");
        let prior = std::fs::read(&path).unwrap();
        gw.cache()
            .write_single("tracker", "get_issue", &hash, &payload, 900, &c.args)
            .unwrap();
        let error = format!("{:#}", gw.call(&c).unwrap_err());
        assert!(error.contains("tool result"), "{error}");
        assert!(!error.contains("private-result-must-not-appear"), "{error}");
        if index == 3 {
            assert!(error.contains("serialized size limit"), "{error}");
        }
        let meta: Value = serde_json::from_slice(
            &std::fs::read(path.with_file_name("page.json.meta.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(meta["complete"], false);
        assert_eq!(std::fs::read(path).unwrap(), prior);
    }
    assert_eq!(server.count("call:get_issue"), 0);
}

#[test]
fn result_validation_applies_to_served_results_and_live_serve_misses() {
    let server = MockMcpServer::start();
    let dirs = TempDir::new("validated-result-serve");
    let policies = validation_policies(
        json!({"resultArtifact":"page","serveFrom":{"fetchedBy":"tracker.list_issues","match":"id"},"onMiss":"fetch_live"}),
        1024,
        json!({"type":"object","required":["title"],"properties":{"title":{"const":"Accepted item"}}}),
    );
    let mut gw = gateway(&server, policies, &dirs, None);
    let source_args = json!({"query":"source"});
    let hash = trollm_mcp::CacheStore::hash_args(&source_args, None, None);
    let good = json!({"id":7,"title":"Accepted item","content":"exact served data"});
    gw.cache()
        .write_single("tracker", "list_issues", &hash, &good, 900, &source_args)
        .unwrap();
    let c = call("tracker", "get_issue", json!({"id":7}));
    assert_eq!(gw.call(&c).unwrap().provenance, Provenance::ServedFromFile);
    assert_eq!(read_result_artifact(&dirs), good);
    let bad = json!({"id":7,"title":"private-served-title"});
    gw.cache()
        .write_single("tracker", "list_issues", &hash, &bad, 900, &source_args)
        .unwrap();
    let error = format!("{:#}", gw.call(&c).unwrap_err());
    assert!(error.contains("tool result validation"), "{error}");
    assert!(!error.contains("private-served-title"), "{error}");
    assert_eq!(server.count("call:get_issue"), 0);
    // A miss goes live and is checked by the same guard, too.
    let error = gw
        .call(&call("tracker", "get_issue", json!({"id":99})))
        .unwrap_err();
    assert!(
        error.to_string().contains("tool result validation"),
        "{error}"
    );
    assert_eq!(server.count("call:get_issue"), 1);
    let meta: Value = serde_json::from_slice(
        &std::fs::read(dirs.path().join("artifacts/page.json.meta.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(meta["complete"], false);
}

#[test]
fn result_validation_failed_live_response_is_not_cached_and_upstream_can_recover() {
    let server = MockMcpServer::start();
    let dirs = TempDir::new("validated-cache-recovery");
    let policies = validation_policies(
        json!({"resultArtifact":"page","cache":{"key":["id"],"ttlS":900}}),
        1024,
        page_result_schema(),
    );
    let mut gw = gateway(&server, policies, &dirs, None);
    let c = call("tracker", "get_issue", json!({"id":7}));
    let hash = trollm_mcp::CacheStore::hash_args(&c.args, Some(&["id".into()]), None);
    server.tool_payloads.lock().unwrap().insert(
        "get_issue".into(),
        json!({"content":"partial data","stats":{"truncated":true,"contentNodes":1}}),
    );
    let error = gw.call(&c).unwrap_err();
    assert!(
        error.to_string().contains("tool result validation"),
        "{error}"
    );
    assert!(gw
        .cache()
        .fresh_entry("tracker", "get_issue", &hash)
        .unwrap()
        .is_none());
    let meta_path = dirs.path().join("artifacts/page.json.meta.json");
    let meta: Value = serde_json::from_slice(&std::fs::read(meta_path).unwrap()).unwrap();
    assert_eq!(meta["complete"], false);
    let good =
        json!({"content":"Complete recovered data","stats":{"truncated":false,"contentNodes":1}});
    server
        .tool_payloads
        .lock()
        .unwrap()
        .insert("get_issue".into(), good.clone());
    assert_eq!(gw.call(&c).unwrap().provenance, Provenance::Live);
    assert_eq!(read_result_artifact(&dirs), good);
    assert_eq!(gw.call(&c).unwrap().provenance, Provenance::Cache);
    assert_eq!(server.count("call:get_issue"), 2);
}

#[test]
fn run_argument_constraints_enforce_selection_and_catalog_without_rewriting() {
    let server = MockMcpServer::start_with_tools_response(json!({"result":{"tools":[
        {"name":"list_tabs","inputSchema":{"type":"object"}},
        {"name":"act","inputSchema":{"type":"object"}},
        {"name":"read_page","inputSchema":{"type":"object","required":["tabId"],"properties":{"tabId":{"type":"number"}}}}
    ]}}));
    let dirs = TempDir::new("run-argument-constraints");
    let mut gw = gateway(&server, PolicySet::default(), &dirs, None)
        .with_argument_constraints(std::collections::BTreeMap::from([
            ("tracker.list_tabs".into(), json!({"type":"object","required":["browser"],"properties":{"browser":{"const":"chosen"}}})),
            ("tracker.act".into(), json!({"type":"object","required":["browser","action","target"],"properties":{"browser":{"const":"chosen"},"action":{"const":"open"},"target":{"const":"https://example.org/page"}},"not":{"anyOf":[{"required":["sessionId"]},{"required":["tabId"]}]}})),
            ("tracker.read_page".into(), json!({"type":"object","required":["browser","tabId"],"properties":{"browser":{"const":"chosen"}},"not":{"required":["sessionId"]}})),
        ])).unwrap();
    for (tool, args) in [
        ("list_tabs", json!({})),
        ("list_tabs", json!({"browser":"other"})),
        (
            "act",
            json!({"browser":"other","action":"open","target":"https://example.org/page"}),
        ),
        (
            "act",
            json!({"browser":"chosen","action":"navigate","target":"https://example.org/page"}),
        ),
        (
            "act",
            json!({"browser":"chosen","action":"open","target":"https://other.example/page"}),
        ),
        (
            "act",
            json!({"browser":"chosen","action":"open","target":"https://example.org/page","tabId":2}),
        ),
        (
            "read_page",
            json!({"browser":"chosen","tabId":2,"sessionId":"other-session"}),
        ),
        (
            "read_page",
            json!({"browser":"chosen","tabId":"wrong-type"}),
        ),
    ] {
        let c = call("tracker", tool, args);
        assert!(gw.validate_call(&c).is_err());
        assert!(gw.call(&c).is_err());
        assert_eq!(server.count(&format!("call:{tool}")), 0);
    }
    for (tool, args) in [
        ("list_tabs", json!({"browser":"chosen"})),
        (
            "act",
            json!({"browser":"chosen","action":"open","target":"https://example.org/page"}),
        ),
        ("read_page", json!({"browser":"chosen","tabId":2})),
    ] {
        let original = args.clone();
        let c = call("tracker", tool, args);
        gw.validate_call(&c).unwrap();
        gw.call(&c).unwrap();
        assert_eq!(c.args, original);
        assert_eq!(server.count(&format!("call:{tool}")), 1);
    }
}

#[test]
fn run_argument_constraints_compile_offline_and_reject_invalid_keys() {
    let dirs = TempDir::new("argument-constraints-compile");
    let build = || {
        McpGateway::new(
            vec![ServerConfig::new("tracker", "https://example.invalid/mcp")],
            PolicySet::default(),
            dirs.path().join("cache"),
            dirs.path().join("artifacts"),
            None,
        )
        .unwrap()
    };
    for (key, schema) in [
        ("tracker.act", json!({"type":"not-valid"})),
        (
            "tracker.act",
            json!({"$ref":"https://example.invalid/schema"}),
        ),
        ("unknown.act", json!({})),
        ("tracker", json!({})),
    ] {
        assert!(build()
            .with_argument_constraints(std::collections::BTreeMap::from([(key.into(), schema)]))
            .is_err());
    }
}

#[test]
fn near_cap_remote_error_is_bounded_and_never_published() {
    let server = MockMcpServer::start();
    let dirs = TempDir::new("near-cap-error");
    let policies = PolicySet::from_yaml_str(
        "tracker.summarize: {class: read, cache: {key: [query], ttlS: 900}, resultArtifact: page}",
    )
    .unwrap();
    let mut gw = gateway(&server, policies, &dirs, None);
    let request = call(
        "tracker",
        "summarize",
        json!({"query":"test", "toolError":"near_cap"}),
    );
    for _ in 0..2 {
        let diagnostic = format!("{:#}", gw.call(&request).unwrap_err());
        assert!(diagnostic.contains("summarize"));
        assert!(diagnostic.contains("isError"));
        assert!(
            diagnostic.len() < 512,
            "diagnostic length {}",
            diagnostic.len()
        );
        assert!(!diagnostic.contains("credential-secret"));
    }
    assert_eq!(server.count("call:summarize"), 2);
    assert!(!dirs.path().join("artifacts/page.json").exists());
    assert!(!dirs.path().join("artifacts/page.json.meta.json").exists());
}

#[test]
fn audit_failure_after_mutation_blocks_every_subsequent_dispatch() {
    let server = MockMcpServer::start();
    let dirs = TempDir::new("audit-after-mutation");
    let attempts = Arc::new(Mutex::new(0));
    let captured = attempts.clone();
    let sink: EventSink = Box::new(move |event| {
        *captured.lock().unwrap() += 1;
        if event.kind() == "live_call" {
            anyhow::bail!("injected audit flush failure");
        }
        Ok(())
    });
    let mut gw = gateway(&server, fixture_policies(), &dirs, Some(sink));
    let mutation = call("tracker", "close_issue", json!({"id":1}));
    assert!(gw
        .call(&mutation)
        .unwrap_err()
        .to_string()
        .contains("proxy audit write failed"));
    assert_eq!(server.count("call:close_issue"), 1);
    let requests = server.request_auth.lock().unwrap().len();
    for request in [
        &mutation,
        &call("tracker", "list_issues", json!({"query":"test"})),
    ] {
        assert!(gw
            .call(request)
            .unwrap_err()
            .to_string()
            .contains("further MCP dispatch prohibited"));
    }
    assert!(gw.list_tools().is_err());
    // A replacement sink cannot erase the lifetime failure latch.
    gw.set_event_sink(Box::new(|_| Ok(())));
    assert!(gw.call(&mutation).is_err());
    assert_eq!(server.request_auth.lock().unwrap().len(), requests);
    assert_eq!(*attempts.lock().unwrap(), 2);
}

#[test]
fn audit_intent_failure_prevents_mutation_dispatch() {
    let server = MockMcpServer::start();
    let dirs = TempDir::new("audit-before-mutation");
    let mut gw = gateway(
        &server,
        fixture_policies(),
        &dirs,
        Some(Box::new(|_| anyhow::bail!("injected write failure"))),
    );
    assert!(gw
        .call(&call("tracker", "close_issue", json!({"id":1})))
        .is_err());
    assert_eq!(server.count("call:close_issue"), 0);
}
