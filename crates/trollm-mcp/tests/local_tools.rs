//! Builtin dispatch uses the ordinary gateway admission and publication path.

mod common;

use common::{MockMcpServer, TempDir};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use trollm_mcp::{
    policy::ToolClass, EventSink, McpGateway, PolicySet, Provenance, ProxyEvent, ServerConfig,
    ToolCall, ToolGateway,
};

fn gateway(dirs: &TempDir, policy: Value, sink: Option<EventSink>) -> McpGateway {
    McpGateway::new(
        vec![],
        policies(policy),
        dirs.path().join("cache"),
        dirs.path().join("artifacts"),
        sink,
    )
    .unwrap()
}

fn policies(policy: Value) -> PolicySet {
    PolicySet::from_yaml_str(&json!({"local.calculate": policy}).to_string()).unwrap()
}

fn call(operation: &str, left: &str, right: &str) -> ToolCall {
    ToolCall {
        server: "local".into(),
        tool: "calculate".into(),
        args: json!({"operation": operation, "left": left, "right": right}),
    }
}

fn read_json(path: &Path) -> Value {
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

#[test]
fn builtin_discovery_and_dispatch_need_no_remote_configuration() {
    let dirs = TempDir::new("local-discovery");
    let mut gw = McpGateway::new(
        vec![],
        PolicySet::default(),
        dirs.path().join("cache"),
        dirs.path().join("artifacts"),
        None,
    )
    .unwrap();
    let tools = gw.list_tools().unwrap();
    assert_eq!(tools.len(), 2);
    assert_eq!(tools[0].server, "local");
    assert_eq!(tools[0].name, "calculate");
    assert_eq!(tools[0].input_schema["type"], "object");
    assert_eq!(gw.tool_class("local", "calculate"), ToolClass::Read);
    let result = gw.call(&call("add", "0.1", "0.2")).unwrap();
    assert_eq!(result.provenance, Provenance::Local);
    assert_eq!(result.content_for_model["result"], "0.3");
    assert!(result.artifacts.is_empty());
    assert!(!dirs.path().join("artifacts").exists());
}

#[test]
fn builtin_rejects_malformed_and_unknown_calls_without_audit_or_artifacts() {
    let dirs = TempDir::new("local-admission");
    let events = Arc::new(Mutex::new(Vec::new()));
    let recorded = events.clone();
    let mut gw = gateway(
        &dirs,
        json!({"resultArtifact": "calculation"}),
        Some(Box::new(move |event| {
            recorded.lock().unwrap().push(event);
            Ok(())
        })),
    );
    for args in [
        json!(null),
        json!({}),
        json!({"operation": "add", "left": 1, "right": "2"}),
        json!({"operation": "power", "left": "1", "right": "2"}),
        json!({"operation": "add", "left": "1e3", "right": "2"}),
        json!({"operation": "add", "left": "1", "right": "2", "extra": true}),
    ] {
        let mut request = call("add", "1", "2");
        request.args = args;
        assert!(gw.validate_call(&request).is_err());
        assert!(gw.call(&request).is_err());
    }
    let mut unknown = call("add", "1", "2");
    unknown.tool = "missing".into();
    assert!(gw.validate_call(&unknown).is_err());
    assert!(gw.call(&unknown).is_err());
    assert!(events.lock().unwrap().is_empty());
    assert!(!dirs.path().join("artifacts").exists());
}

#[test]
fn builtin_namespace_and_unsupported_policies_fail_at_construction() {
    let dirs = TempDir::new("local-policy");
    assert!(McpGateway::new(
        vec![ServerConfig::new("local", "https://example.invalid/mcp")],
        PolicySet::default(),
        dirs.path().join("cache"),
        dirs.path().join("artifacts"),
        None,
    )
    .is_err());
    for policy in [
        json!({"class": "mutation"}),
        json!({"cache": {"ttlS": 30}}),
        json!({"paginate": {"param": "cursor", "deliver": "first_page", "backgroundFetch": "all"}}),
        json!({"serveFrom": {"fetchedBy": "remote.list", "match": "id"}}),
        json!({"onMiss": "fail"}),
        json!({"passthrough": true}),
    ] {
        assert!(
            McpGateway::new(
                vec![],
                policies(policy.clone()),
                dirs.path().join("cache"),
                dirs.path().join("artifacts"),
                None,
            )
            .is_err(),
            "{policy}"
        );
    }
    let unknown = PolicySet::from_yaml_str("local.missing: {}").unwrap();
    assert!(McpGateway::new(
        vec![],
        unknown,
        Default::default(),
        Default::default(),
        None
    )
    .is_err());
    // Reserved transport defaults remain accepted without a remote transport.
    gateway(&dirs, json!({"timeoutS": 5, "retry": {}}), None);
}

#[test]
fn runtime_flow_namespace_is_never_a_remote_server_policy_or_gateway_action() {
    let dirs = TempDir::new("flow-reserved");
    assert!(McpGateway::new(
        vec![ServerConfig::new("flow", "https://example.invalid/mcp")],
        PolicySet::default(),
        dirs.path().join("cache"),
        dirs.path().join("artifacts"),
        None,
    )
    .is_err());
    for tool in ["choose", "unknown"] {
        assert!(
            PolicySet::from_yaml_str(&json!({format!("flow.{tool}"): {}}).to_string()).is_err()
        );
        let mut gw = gateway(&dirs, json!({}), None);
        let call = ToolCall {
            server: "flow".into(),
            tool: tool.into(),
            args: json!({"route": "done"}),
        };
        assert!(gw.validate_call(&call).is_err());
        assert!(gw.call(&call).is_err());
    }
    assert!(PolicySet::from_yaml_str(
        &json!({"remote.read": {
            "serveFrom": {"fetchedBy": "flow.choose", "match": "id"}
        }})
        .to_string()
    )
    .is_err());
    assert!(!dirs.path().join("artifacts").exists());
}

#[test]
fn builtin_argument_constraints_apply_to_validate_and_direct_call() {
    let dirs = TempDir::new("local-constraints");
    let constraints = BTreeMap::from([(
        "local.calculate".into(),
        json!({"properties": {"operation": {"const": "compare"}}}),
    )]);
    let mut gw = gateway(&dirs, json!({}), None)
        .with_argument_constraints(constraints)
        .unwrap();
    assert!(gw.validate_call(&call("add", "1", "2")).is_err());
    assert!(gw.call(&call("add", "1", "2")).is_err());
    assert_eq!(
        gw.call(&call("compare", "1", "2"))
            .unwrap()
            .content_for_model["result"],
        "less"
    );
    assert!(gateway(&dirs, json!({}), None)
        .with_argument_constraints(BTreeMap::from([("local.missing".into(), json!(true))]))
        .is_err());
}

#[test]
fn builtin_preserves_exact_artifact_before_shape_and_invalidates_failed_replacement() {
    let dirs = TempDir::new("local-artifacts");
    let mut gw = gateway(
        &dirs,
        json!({
            "resultArtifact": "calculation",
            "resultValidation": {"maxBytes": 1024, "schema": {
                "type": "object", "properties": {"result": {"const": "0.3"}}
            }},
            "shape": {"strip": ["result"], "toFile": true}
        }),
        None,
    );
    let result = gw.call(&call("add", "0.1", "0.2")).unwrap();
    let path = dirs.path().join("artifacts/calculation.json");
    let meta = dirs.path().join("artifacts/calculation.json.meta.json");
    let expected = json!({
        "format": "trollm.local.calculate.v1", "operation": "add",
        "left": "0.1", "right": "0.2", "result": "0.3"
    });
    assert_eq!(read_json(&path), expected);
    assert_eq!(read_json(&meta)["complete"], true);
    assert_eq!(result.provenance, Provenance::Local);
    assert!(result.artifacts.contains(&path));
    assert!(result.content_for_model["result"]["path"].is_string());
    assert!(gw.call(&call("add", "1", "2")).is_err());
    assert_eq!(read_json(&meta)["complete"], false);
    assert_eq!(read_json(&path), expected);
}

#[test]
fn builtin_local_audit_is_redacted_and_failure_blocks_dispatch() {
    let dirs = TempDir::new("local-audit");
    let events = Arc::new(Mutex::new(Vec::new()));
    let recorded = events.clone();
    let mut gw = gateway(
        &dirs,
        json!({}),
        Some(Box::new(move |event| {
            recorded.lock().unwrap().push(event);
            Ok(())
        })),
    );
    gw.call(&call("multiply", "12345.678901", "1")).unwrap();
    let events = events.lock().unwrap();
    assert!(matches!(events[0], ProxyEvent::LocalCall { .. }));
    assert_eq!(events[0].kind(), "local_call");
    let event = serde_json::to_value(&events[0]).unwrap();
    assert_eq!(event["type"], "local_call");
    assert!(event.get("args").is_none());
    assert!(!event.to_string().contains("12345.678901"));
    drop(events);
    gw.set_event_sink(Box::new(|_| anyhow::bail!("audit unavailable")));
    // Division would fail in the calculator; the audit failure takes priority.
    let error = gw.call(&call("divide", "1", "0")).unwrap_err();
    assert!(error.to_string().contains("proxy audit write failed"));
    gw.set_event_sink(Box::new(|_| Ok(())));
    assert!(gw.call(&call("add", "1", "2")).is_err());
    assert!(gw.list_tools().is_err());
}

#[test]
fn builtin_dispatch_does_not_contact_configured_remote_server() {
    let server = MockMcpServer::start();
    let dirs = TempDir::new("local-remote-routing");
    let mut gw = McpGateway::new(
        vec![ServerConfig::for_local_test("tracker", server.url.clone())],
        PolicySet::default(),
        dirs.path().join("cache"),
        dirs.path().join("artifacts"),
        None,
    )
    .unwrap();
    gw.call(&call("add", "1", "2")).unwrap();
    assert!(server.counts.lock().unwrap().is_empty());
    let remote = ToolCall {
        server: "tracker".into(),
        tool: "get_issue".into(),
        args: json!({"id": "T-1"}),
    };
    assert_eq!(gw.call(&remote).unwrap().provenance, Provenance::Live);
    assert_eq!(server.count("list"), 1);
    assert_eq!(server.count("call:get_issue"), 1);
}
