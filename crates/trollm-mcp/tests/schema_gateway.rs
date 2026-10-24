//! Schema enforcement at the real MCP gateway boundary, using local wire fixtures.
mod common;

use common::{MockMcpServer, TempDir};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use trollm_mcp::{EventSink, McpGateway, PolicySet, ServerConfig, ToolCall, ToolGateway};

fn call(server: &str, tool: &str, args: Value) -> ToolCall {
    ToolCall {
        server: server.into(),
        tool: tool.into(),
        args,
    }
}

fn schema_server(tool: &str, schema: Value) -> MockMcpServer {
    MockMcpServer::start_with_tools_response(json!({"result": {"tools": [
        {"name": tool, "inputSchema": schema}
    ]}}))
}

fn integer_id_schema() -> Value {
    json!({"type":"object", "properties":{"id":{"type":"integer"}},
        "required":["id"], "additionalProperties":false})
}

fn gateway(
    server: &MockMcpServer,
    dirs: &TempDir,
    policies: &str,
    sink: Option<EventSink>,
) -> McpGateway {
    McpGateway::new(
        vec![ServerConfig::for_local_test("tracker", &server.url)],
        PolicySet::from_yaml_str(policies).unwrap(),
        dirs.path().join("cache"),
        dirs.path().join("artifacts"),
        sink,
    )
    .unwrap()
}

#[test]
fn local_guidance_and_dispatch_ignore_an_irrelevant_failing_server() {
    let server = MockMcpServer::start_with_tools_response(json!({
        "error": {"code": -32603, "message": "fixture discovery failure"}
    }));
    let dirs = TempDir::new("local-guidance-no-discovery");
    let mut gw = gateway(&server, &dirs, "{}", None);
    let tools = gw.available_tools(&[
        "local.calculate".into(),
        "tracker.get_issue".into(),
        "local.calculate_extra".into(),
    ]);
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].server, "local");
    assert_eq!(tools[0].name, "calculate");
    assert_eq!(tools[0].input_schema["type"], "object");
    assert!(gw.available_tools(&[]).is_empty());
    let result = gw
        .call(&call(
            "local",
            "calculate",
            json!({"operation":"add", "left":"0.1", "right":"0.2"}),
        ))
        .unwrap();
    assert_eq!(result.content_for_model["result"], "0.3");
    assert!(server.counts.lock().unwrap().is_empty());
}

#[test]
fn remote_guidance_uses_only_catalogs_discovered_by_dispatch() {
    let schema = integer_id_schema();
    let server = schema_server("get_issue", schema.clone());
    let dirs = TempDir::new("cached-guidance");
    let mut gw = gateway(&server, &dirs, "{}", None);
    let grants = vec!["tracker.get_issue".into(), "tracker.get_issue_extra".into()];
    assert!(gw.available_tools(&grants).is_empty());
    assert!(server.counts.lock().unwrap().is_empty());

    gw.call(&call("tracker", "get_issue", json!({"id":1})))
        .unwrap();
    assert_eq!(server.count("list"), 1);
    assert_eq!(server.count("call:get_issue"), 1);
    let before = server.counts.lock().unwrap().clone();
    let tools = gw.available_tools(&grants);
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].server, "tracker");
    assert_eq!(tools[0].name, "get_issue");
    assert_eq!(tools[0].input_schema, schema);
    assert!(gw
        .available_tools(&["tracker.get_issue_extra".into()])
        .is_empty());
    assert_eq!(*server.counts.lock().unwrap(), before);

    // Guidance does not replace final wire-schema enforcement.
    assert!(gw
        .call(&call("tracker", "get_issue", json!({"id":"1"})))
        .is_err());
    assert_eq!(server.count("call:get_issue"), 1);
}

#[test]
fn invalid_mutation_is_rejected_before_dispatch_or_intent_and_valid_mutation_calls_once() {
    let server = schema_server("close_issue", integer_id_schema());
    let dirs = TempDir::new("schema-mutation");
    let events = Arc::new(Mutex::new(Vec::new()));
    let captured = events.clone();
    let sink: EventSink = Box::new(move |event| {
        captured.lock().unwrap().push(event.kind().to_owned());
        Ok(())
    });
    let mut gw = gateway(
        &server,
        &dirs,
        "tracker.close_issue: {class: mutation, passthrough: true}",
        Some(sink),
    );
    let invalid = call("tracker", "close_issue", json!({"id":"3"}));
    assert!(gw.validate_call(&invalid).is_err());
    assert!(
        gw.call(&invalid).is_err(),
        "direct call must enforce validation too"
    );
    assert_eq!(server.count("call:close_issue"), 0);
    assert!(
        events.lock().unwrap().is_empty(),
        "invalid mutations emit no intent"
    );
    let valid = call("tracker", "close_issue", json!({"id":3}));
    gw.validate_call(&valid).unwrap();
    gw.call(&valid).unwrap();
    assert_eq!(server.count("call:close_issue"), 1);
    assert_eq!(
        *events.lock().unwrap(),
        vec![
            "mutation_passthrough",
            "response_size",
            "live_call",
            "response_size"
        ]
    );
    assert_eq!(server.count("list"), 1);
}

#[test]
fn direct_call_without_preflight_cannot_bypass_schema() {
    let server = schema_server("close_issue", integer_id_schema());
    let dirs = TempDir::new("schema-direct");
    let mut gw = gateway(
        &server,
        &dirs,
        "tracker.close_issue: {class: mutation}",
        None,
    );
    assert!(gw.call(&call("tracker", "close_issue", json!({}))).is_err());
    assert_eq!(server.count("call:close_issue"), 0);
    assert_eq!(server.count("list"), 1);
}

#[test]
fn repeated_validation_listing_and_calls_reuse_one_catalog() {
    let server = schema_server("close_issue", integer_id_schema());
    let dirs = TempDir::new("schema-frozen");
    let mut gw = gateway(
        &server,
        &dirs,
        "tracker.close_issue: {class: mutation}",
        None,
    );
    let c = call("tracker", "close_issue", json!({"id":1}));
    for _ in 0..3 {
        gw.validate_call(&c).unwrap();
        gw.list_tools().unwrap();
        gw.call(&c).unwrap();
    }
    assert_eq!(server.count("list"), 1);
    assert_eq!(server.count("call:close_issue"), 3);
}

#[test]
fn malformed_catalogs_fail_listing_validation_and_direct_calls_closed() {
    let tool = json!({"name":"close_issue", "inputSchema":integer_id_schema()});
    let cases = [
        (
            "missing-schema",
            json!({"result":{"tools":[{"name":"close_issue"}]}}),
        ),
        (
            "malformed-schema",
            json!({"result":{"tools":[{"name":"close_issue", "inputSchema":{"type":"not-a-type"}}]}}),
        ),
        (
            "duplicate-tool",
            json!({"result":{"tools":[tool.clone(),tool]}}),
        ),
        (
            "discovery-error",
            json!({"error":{"code":-32603,"message":"fixture discovery failure"}}),
        ),
        ("missing-tools", json!({"result":{}})),
    ];
    for (tag, response) in cases {
        let server = MockMcpServer::start_with_tools_response(response);
        let dirs = TempDir::new(tag);
        let mut gw = gateway(
            &server,
            &dirs,
            "tracker.close_issue: {class: mutation}",
            None,
        );
        let c = call("tracker", "close_issue", json!({"id":1}));
        assert!(gw.list_tools().is_err(), "{tag}");
        assert!(gw.validate_call(&c).is_err(), "{tag}");
        assert!(gw.call(&c).is_err(), "{tag}");
        assert_eq!(server.count("call:close_issue"), 0, "{tag}");
    }
}

#[test]
fn unknown_tools_never_dispatch_even_with_a_policy() {
    let server = MockMcpServer::start();
    let dirs = TempDir::new("schema-unknown");
    let mut gw = gateway(&server, &dirs, "tracker.unknown: {class: mutation}", None);
    let c = call("tracker", "unknown", json!({}));
    assert!(gw.validate_call(&c).is_err());
    assert!(gw.call(&c).is_err());
    assert_eq!(server.count("call:unknown"), 0);
}

#[test]
fn equal_tool_names_on_distinct_servers_do_not_share_schemas() {
    let first = schema_server("close_issue", integer_id_schema());
    let second = schema_server(
        "close_issue",
        json!({"type":"object", "properties":{"id":{"type":"string"}}, "required":["id"], "additionalProperties":false}),
    );
    let dirs = TempDir::new("schema-servers");
    let mut gw = McpGateway::new(
        vec![
            ServerConfig::for_local_test("first", &first.url),
            ServerConfig::for_local_test("second", &second.url),
        ],
        PolicySet::from_yaml_str(
            "first.close_issue: {class: mutation}\nsecond.close_issue: {class: mutation}",
        )
        .unwrap(),
        dirs.path().join("cache"),
        dirs.path().join("artifacts"),
        None,
    )
    .unwrap();
    let number = call("first", "close_issue", json!({"id":1}));
    let text = call("second", "close_issue", json!({"id":"one"}));
    gw.validate_call(&number).unwrap();
    gw.validate_call(&text).unwrap();
    assert!(gw
        .call(&call("first", "close_issue", text.args.clone()))
        .is_err());
    assert!(gw
        .call(&call("second", "close_issue", number.args.clone()))
        .is_err());
    gw.call(&number).unwrap();
    gw.call(&text).unwrap();
    assert_eq!(first.count("list"), 1);
    assert_eq!(second.count("list"), 1);
    assert_eq!(first.count("call:close_issue"), 1);
    assert_eq!(second.count("call:close_issue"), 1);
}

fn cursor_schema() -> Value {
    json!({"type":"object", "properties":{"query":{"type":"string"},"cursor":{"type":"integer","minimum":1,"maximum":1}}, "additionalProperties":false})
}
const PAGINATION: &str = "tracker.list_issues: {class: read, cache: {key: [query], ttlS: 900}, paginate: {param: cursor, deliver: first_page, backgroundFetch: all}}";

#[test]
fn synthesized_cursor_must_validate_before_the_next_request() {
    let server = schema_server("list_issues", cursor_schema());
    let dirs = TempDir::new("schema-cursor");
    let mut gw = gateway(&server, &dirs, PAGINATION, None);
    let c = call("tracker", "list_issues", json!({"query":"open"}));
    gw.validate_call(&c).unwrap();
    assert!(
        gw.call(&c).is_err(),
        "server cursor 2 violates the discovered maximum"
    );
    assert_eq!(server.count("call:list_issues"), 1);
    assert!(!dirs
        .path()
        .join("artifacts/tracker.list_issues.latest.jsonl")
        .exists());
}

#[test]
fn model_page_selector_must_be_a_positive_integer_before_dispatch() {
    let server = schema_server("list_issues", json!({"type":"object"}));
    let dirs = TempDir::new("schema-page-selector");
    let mut gw = gateway(&server, &dirs, PAGINATION, None);
    for cursor in [json!(0), json!(-1), json!("2"), json!(1.5), json!(null)] {
        let c = call(
            "tracker",
            "list_issues",
            json!({"query":"open", "cursor":cursor}),
        );
        assert!(gw.validate_call(&c).is_err());
        assert!(gw.call(&c).is_err());
    }
    assert_eq!(server.count("call:list_issues"), 0);
}

#[test]
fn catalog_tool_count_is_bounded_before_any_dispatch() {
    let tools: Vec<Value> = (0..1025)
        .map(|i| json!({"name":format!("tool_{i}"),"inputSchema":{"type":"object"}}))
        .collect();
    let server = MockMcpServer::start_with_tools_response(json!({"result":{"tools":tools}}));
    let dirs = TempDir::new("schema-tool-bound");
    let mut gw = gateway(&server, &dirs, "tracker.tool_0: {class: mutation}", None);
    assert!(gw.list_tools().is_err());
    assert!(gw.call(&call("tracker", "tool_0", json!({}))).is_err());
    assert_eq!(server.count("call:tool_0"), 0);
}

#[test]
fn positive_model_page_is_stripped_before_validating_the_first_wire_arguments() {
    let server = schema_server("list_issues", cursor_schema());
    let dirs = TempDir::new("schema-page-strip");
    let mut gw = gateway(&server, &dirs, PAGINATION, None);
    let c = call(
        "tracker",
        "list_issues",
        json!({"query":"open", "cursor":2}),
    );
    // This 2 is a local requested page, not the cursor of the first wire request.
    gw.validate_call(&c).unwrap();
    assert!(
        gw.call(&c).is_err(),
        "the subsequent server cursor must still validate"
    );
    assert_eq!(server.count("call:list_issues"), 1);
}

#[test]
fn paged_catalog_routes_cursor_and_freezes_second_page_tools() {
    let server = MockMcpServer::start_with_tools_responses(vec![
        json!({"result":{"tools":[{"name":"summarize","inputSchema":{"type":"object"}}],"nextCursor":"more"}}),
        json!({"result":{"tools":[{"name":"close_issue","inputSchema":integer_id_schema()}]}}),
    ]);
    let dirs = TempDir::new("schema-list-pages");
    let mut gw = gateway(
        &server,
        &dirs,
        "tracker.close_issue: {class: mutation}",
        None,
    );
    let tools = gw.list_tools().unwrap();
    assert_eq!(
        tools.iter().filter(|tool| tool.server == "tracker").count(),
        2
    );
    assert_eq!(
        tools
            .iter()
            .filter(|tool| tool.name == "close_issue")
            .count(),
        1
    );
    let c = call("tracker", "close_issue", json!({"id":1}));
    gw.validate_call(&c).unwrap();
    gw.call(&c).unwrap();
    gw.list_tools().unwrap();
    assert_eq!(server.count("list"), 2);
    assert_eq!(server.count("call:close_issue"), 1);
    assert_eq!(
        *server.list_params.lock().unwrap(),
        vec![json!({}), json!({"cursor":"more"})]
    );
}

#[test]
fn repeated_catalog_cursor_fails_before_another_request() {
    let server = MockMcpServer::start_with_tools_response(
        json!({"result":{"tools":[],"nextCursor":"same"}}),
    );
    let dirs = TempDir::new("schema-list-cycle");
    let gw = gateway(&server, &dirs, "{}", None);
    assert!(gw.list_tools().is_err());
    assert_eq!(server.count("list"), 2);
    assert_eq!(
        *server.list_params.lock().unwrap(),
        vec![json!({}), json!({"cursor":"same"})]
    );
}

#[test]
fn non_string_catalog_cursor_fails_before_another_request() {
    let server =
        MockMcpServer::start_with_tools_response(json!({"result":{"tools":[],"nextCursor":2}}));
    let dirs = TempDir::new("schema-list-cursor-type");
    let gw = gateway(&server, &dirs, "{}", None);
    assert!(gw.list_tools().is_err());
    assert_eq!(server.count("list"), 1);
}

#[test]
fn catalog_page_limit_prevents_a_seventeenth_request() {
    let responses = (1..=16)
        .map(|page| json!({"result":{"tools":[],"nextCursor":format!("page-{page}")}}))
        .collect();
    let server = MockMcpServer::start_with_tools_responses(responses);
    let dirs = TempDir::new("schema-list-page-bound");
    let gw = gateway(&server, &dirs, "{}", None);
    assert!(gw.list_tools().is_err());
    assert_eq!(server.count("list"), 16);
    let params = server.list_params.lock().unwrap();
    assert_eq!(params[0], json!({}));
    for page in 1..16 {
        assert_eq!(params[page], json!({"cursor":format!("page-{page}")}));
    }
}

#[test]
fn oversized_catalog_is_rejected_even_when_only_description_is_large() {
    let server = MockMcpServer::start_with_tools_response(json!({"result":{"tools":[
        {"name":"close_issue","inputSchema":integer_id_schema(),"description":"x".repeat(4 * 1024 * 1024)}
    ]}}));
    let dirs = TempDir::new("schema-list-byte-bound");
    let mut gw = gateway(
        &server,
        &dirs,
        "tracker.close_issue: {class: mutation}",
        None,
    );
    assert!(gw.list_tools().is_err());
    assert!(gw
        .call(&call("tracker", "close_issue", json!({"id":1})))
        .is_err());
    assert_eq!(server.count("call:close_issue"), 0);
}
