//! Scripted filesystem containment checks; no real MCP service is involved.
mod common;

use common::{MockMcpServer, TempDir};
use serde_json::{json, Value};
use std::path::Path;
use trollm_mcp::{
    CacheStore, McpGateway, PolicySet, Provenance, ServerConfig, ToolCall, ToolGateway,
};

fn assert_tree_under(root: &Path, path: &Path) {
    assert!(path
        .canonicalize()
        .unwrap()
        .starts_with(root.canonicalize().unwrap()));
    if path.is_dir() {
        for entry in std::fs::read_dir(path).unwrap() {
            assert_tree_under(root, &entry.unwrap().path());
        }
    }
}

#[test]
fn cache_encodes_identifiers_on_writes_and_reads_without_legacy_fallback() {
    let dirs = TempDir::new("paths");
    let root = dirs.path().join("assigned/cache");
    let outside = dirs.path().join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    let hash = CacheStore::hash_args(&json!({}), None, None);
    // A stale unsafe legacy path must never supply a hit after the repair.
    std::fs::write(outside.join(format!("{hash}.jsonl")), b"{\"stale\":true}\n").unwrap();
    std::fs::write(outside.join("sentinel"), b"unchanged").unwrap();
    std::fs::write(
        outside.join(format!("{hash}.meta.json")),
        serde_json::to_vec(&json!({
            "fetchedAt": trollm_mcp::cache::now_secs(), "ttlS": 900, "complete": true,
            "pages": 1, "totalItems": 1, "sourceArgs": {}
        }))
        .unwrap(),
    )
    .unwrap();
    let store = CacheStore::new(&root);
    let absolute = outside.to_str().unwrap();
    let names = [
        absolute,
        "../../../escape",
        "a/b",
        "a\\b",
        ".",
        "..",
        "",
        "~encoded",
    ];
    for name in names {
        for (server, tool) in [("server", name), (name, "tool")] {
            assert!(store.fresh_entry(server, tool, &hash).unwrap().is_none());
            store
                .write_single(server, tool, &hash, &json!({"id": 7}), 900, &json!({}))
                .unwrap();
            assert_eq!(
                store.read_items(server, tool, &hash).unwrap(),
                vec![json!({"id": 7})]
            );
            assert_eq!(store.fresh_entries_for_tool(server, tool).unwrap().len(), 1);
            store
                .begin_paginated(
                    server,
                    tool,
                    &hash,
                    "items",
                    &[json!({"id": 8})],
                    900,
                    &json!({}),
                )
                .unwrap();
            store
                .append_page(server, tool, &hash, &[json!({"id": 9})])
                .unwrap();
            store
                .finalize_paginated(server, tool, &hash, 900, &json!({}))
                .unwrap();
            assert_eq!(
                store.read_page(server, tool, &hash, 2).unwrap().unwrap().0,
                vec![json!({"id": 9})]
            );
        }
    }
    assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 3);
    assert_eq!(
        std::fs::read(outside.join("sentinel")).unwrap(),
        b"unchanged"
    );
    assert!(!dirs.path().join("escape").exists());
    assert_tree_under(&root, &root);
    for bad_hash in [absolute, "../../../escape", "a/b", "", "ABC"] {
        assert!(store
            .write_single("server", "tool", bad_hash, &json!({}), 900, &json!({}))
            .is_err());
        assert!(store.read_items("server", "tool", bad_hash).is_err());
        assert!(store.fresh_entry("server", "tool", bad_hash).is_err());
        assert!(store
            .begin_paginated("server", "tool", bad_hash, "items", &[], 900, &json!({}))
            .is_err());
    }
}

fn policies(value: Value) -> PolicySet {
    PolicySet::from_yaml_str(&value.to_string()).unwrap()
}

#[test]
fn gateway_preserves_remote_names_and_contains_cache_worklists_and_shapes() {
    let dirs = TempDir::new("gateway-paths");
    let outside = dirs.path().join("outside");
    for unsafe_name in [outside.to_str().unwrap(), "../../../escape", "a/b", "a\\b"] {
        for (server_name, tool) in [("tracker", unsafe_name), (unsafe_name, "list_issues")] {
            let server = MockMcpServer::start_with_tools_response(json!({"result": {"tools": [
                {"name": tool, "inputSchema": {"type": "object"}},
                {"name": "get_issue", "inputSchema": {"type": "object"}}
            ]}}));
            server.tool_payloads.lock().unwrap().insert(
                tool.to_string(),
                json!({"issues": [{"id": 7, "body": "content"}], "cursor": null}),
            );
            let key = format!("{server_name}.{tool}");
            let get_key = format!("{server_name}.get_issue");
            let assigned = dirs.path().join("assigned");
            let artifacts = assigned.join("artifacts");
            let mut gateway = McpGateway::new(
                vec![ServerConfig::for_local_test(server_name, &server.url)],
                policies(json!({key.clone(): {
                    "cache": {"key": [], "ttlS": 900},
                    "paginate": {"param": "cursor", "deliver": "first_page", "backgroundFetch": "all"},
                    "shape": {"strip": ["body"], "toFile": true}
                }, get_key: {"serveFrom": {"fetchedBy": key, "match": "id"}}})),
                assigned.join("cache"), artifacts.clone(), None,
            ).unwrap();
            let call = ToolCall {
                server: server_name.into(),
                tool: tool.into(),
                args: json!({}),
            };
            let first = gateway.call(&call).unwrap();
            assert_eq!(first.provenance, Provenance::Live);
            for artifact in &first.artifacts {
                assert_tree_under(&artifacts, artifact);
            }
            assert!(first
                .artifacts
                .iter()
                .any(|p| p.to_string_lossy().ends_with("latest.jsonl")));
            assert_eq!(gateway.call(&call).unwrap().provenance, Provenance::Cache);
            assert_eq!(server.count(&format!("call:{tool}")), 1);
            // Existing policy FQNs split on the first dot; dotted server names
            // cannot express a serveFrom source and are outside this change.
            if !server_name.contains('.') {
                let detail = gateway
                    .call(&ToolCall {
                        server: server_name.into(),
                        tool: "get_issue".into(),
                        args: json!({"id": 7}),
                    })
                    .unwrap();
                assert_eq!(detail.provenance, Provenance::ServedFromFile);
                assert_eq!(detail.content_for_model["body"], "content");
            }
            assert!(!outside.exists());
            assert!(!dirs.path().join("escape").exists());
            assert!(!assigned.join("escape").exists());
        }
    }
}

#[test]
fn authored_shape_fields_are_contained_with_nonmatching_and_no_file_controls() {
    let dirs = TempDir::new("field-paths");
    let outside = dirs.path().join("outside");
    for field in [
        outside.to_str().unwrap(),
        "../../../escape/body",
        "a/b",
        "a\\b",
        "..",
        "",
    ] {
        for (matches, to_file) in [(true, true), (false, true), (true, false)] {
            let server = MockMcpServer::start();
            let payload = json!({field: "private body"});
            server
                .tool_payloads
                .lock()
                .unwrap()
                .insert("summarize".into(), payload.clone());
            let artifacts = dirs.path().join("assigned/artifacts");
            let mut gateway = McpGateway::new(
                vec![ServerConfig::for_local_test("tracker", &server.url)],
                policies(
                    json!({"tracker.summarize": {"resultArtifact": "page", "shape": {
                        "strip": [if matches {field} else {"nonmatching"}], "toFile": to_file
                    }}}),
                ),
                dirs.path().join("assigned/cache"),
                artifacts.clone(),
                None,
            )
            .unwrap();
            let result = gateway
                .call(&ToolCall {
                    server: "tracker".into(),
                    tool: "summarize".into(),
                    args: json!({}),
                })
                .unwrap();
            assert_eq!(
                result.artifacts.len(),
                if matches && to_file { 2 } else { 1 }
            );
            for artifact in &result.artifacts {
                assert_tree_under(&artifacts, artifact);
            }
            let raw: Value =
                serde_json::from_slice(&std::fs::read(artifacts.join("page.json")).unwrap())
                    .unwrap();
            assert_eq!(raw, payload);
            if !matches {
                assert_eq!(result.content_for_model, payload);
            }
            if matches && !to_file {
                assert!(result.content_for_model[field].get("path").is_none());
            }
            assert!(!outside.exists());
            assert!(!dirs.path().join("escape").exists());
        }
    }
}
