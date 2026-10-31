mod common;
use common::{MockMcpServer, TempDir};
use serde_json::{json, Value};
use trollm_mcp::{McpGateway, PolicySet, Provenance, ServerConfig, ToolCall, ToolGateway};

fn make(server: &MockMcpServer, dirs: &TempDir, policy: &str) -> McpGateway {
    McpGateway::new(
        vec![ServerConfig::for_local_test("tracker", &server.url)],
        PolicySet::from_yaml_str(policy).unwrap(),
        dirs.path().join("cache"),
        dirs.path().join("artifacts"),
        None,
    )
    .unwrap()
}
fn call(args: Value) -> ToolCall {
    ToolCall {
        server: "tracker".into(),
        tool: "summarize".into(),
        args,
    }
}
fn complete(dirs: &TempDir, name: &str) -> bool {
    let meta: Value = serde_json::from_slice(
        &std::fs::read(dirs.path().join(format!("artifacts/{name}.json.meta.json"))).unwrap(),
    )
    .unwrap();
    meta["complete"] == true
}
#[test]
fn separate_producer_names_preserve_original_and_cache_without_sticky_override() {
    let server = MockMcpServer::start();
    let dirs = TempDir::new("task-artifact-cache");
    let mut gw=make(&server,&dirs,"tracker.summarize: {class: read, resultArtifact: browser-text, cache: {key: [query], ttlS: 900}}");
    let c = call(json!({"query":"original"}));
    let original = gw
        .call_with_result_artifact(&c, Some("selected-original"))
        .unwrap();
    assert_eq!(original.provenance, Provenance::Live);
    let path = dirs.path().join("artifacts/selected-original.json");
    let bytes = std::fs::read(&path).unwrap();
    assert!(original.artifacts.contains(&path));
    let narrow = gw
        .call_with_result_artifact(&call(json!({"query":"narrow"})), Some("price-source"))
        .unwrap();
    assert_eq!(narrow.provenance, Provenance::Live);
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    assert!(complete(&dirs, "selected-original") && complete(&dirs, "price-source"));
    let cached = gw
        .call_with_result_artifact(&c, Some("cached-original"))
        .unwrap();
    assert_eq!(cached.provenance, Provenance::Cache);
    assert_eq!(
        std::fs::read(dirs.path().join("artifacts/cached-original.json")).unwrap(),
        bytes
    );
    let default = gw.call(&c).unwrap();
    assert_eq!(default.provenance, Provenance::Cache);
    assert!(default
        .artifacts
        .contains(&dirs.path().join("artifacts/browser-text.json")));
    assert_eq!(server.count("call:summarize"), 2);
}
#[test]
fn replacement_invalidates_before_remote_error_schema_error_or_write_failure() {
    let server = MockMcpServer::start();
    let dirs = TempDir::new("task-artifact-fail");
    let mut gw=make(&server,&dirs,"tracker.summarize: {class: read, resultArtifact: browser-text, resultValidation: {maxBytes: 4096, schema: {type: object, required: [summary]}}}");
    gw.call_with_result_artifact(&call(json!({"query":"initial"})), Some("selected-original"))
        .unwrap();
    assert!(complete(&dirs, "selected-original"));
    assert!(gw
        .call_with_result_artifact(
            &call(json!({"toolError":"structured"})),
            Some("selected-original")
        )
        .is_err());
    assert!(!complete(&dirs, "selected-original"));
    server
        .tool_payloads
        .lock()
        .unwrap()
        .insert("summarize".into(), json!({"wrong":true}));
    assert!(gw
        .call_with_result_artifact(&call(json!({})), Some("selected-original"))
        .is_err());
    assert!(!complete(&dirs, "selected-original"));
    server.tool_payloads.lock().unwrap().remove("summarize");
    std::fs::create_dir(dirs.path().join("artifacts/write-fails.json")).unwrap();
    assert!(gw
        .call_with_result_artifact(&call(json!({})), Some("write-fails"))
        .is_err());
    assert!(!complete(&dirs, "write-fails"));
}
#[test]
fn destinations_reject_reserved_colliding_and_non_read_policies_before_io() {
    let server = MockMcpServer::start();
    let dirs = TempDir::new("task-artifact-invalid");
    let mut gw=make(&server,&dirs,"tracker.summarize: {class: read, resultArtifact: browser-text}\ntracker.get_issue: {resultArtifact: other-owned}");
    for name in [
        "../outside",
        "page.json",
        "_wrong",
        "calculation",
        "model-failure-any",
        "other-owned",
    ] {
        assert!(
            gw.call_with_result_artifact(&call(json!({})), Some(name))
                .is_err(),
            "{name}"
        );
    }
    assert_eq!(server.count("call:summarize"), 0);
    for policy in [
        "tracker.summarize: {class: mutation, passthrough: true, resultArtifact: browser-text}",
        "tracker.summarize: {class: read}",
    ] {
        let mut gw = make(&server, &dirs, policy);
        assert!(gw
            .call_with_result_artifact(&call(json!({})), Some("selected-original"))
            .is_err());
    }
}

#[test]
fn separate_tools_cannot_claim_the_same_dynamic_destination() {
    let server = MockMcpServer::start();
    let dirs = TempDir::new("task-artifact-owners");
    let mut gw = make(
        &server,
        &dirs,
        "tracker.summarize: {resultArtifact: one}\ntracker.get_issue: {resultArtifact: two}",
    );
    gw.begin_result_artifact_attempt("tracker", "summarize", "selected-original")
        .unwrap();
    assert!(gw
        .begin_result_artifact_attempt("tracker", "get_issue", "selected-original")
        .is_err());
    gw.begin_result_artifact_attempt("tracker", "summarize", "selected-original")
        .unwrap();
    assert_eq!(server.count("call:summarize"), 0);
}
