//! Numeric-only compact UTF-8 response telemetry across gateway strategies.
mod common;
use common::{MockMcpServer, TempDir};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use trollm_mcp::{EventSink, McpGateway, PolicySet, ServerConfig, ToolCall, ToolGateway};

fn bytes(v: &Value) -> u64 {
    serde_json::to_vec(v).unwrap().len() as u64
}

#[test]
fn response_sizes_cover_live_cache_served_local_pages_and_errors() {
    let server = MockMcpServer::start();
    let dirs = TempDir::new("response-sizes");
    let events = Arc::new(Mutex::new(Vec::<Value>::new()));
    let captured = events.clone();
    let sink: EventSink = Box::new(move |e| {
        captured.lock().unwrap().push(serde_json::to_value(e)?);
        Ok(())
    });
    let policies = PolicySet::from_yaml_str(
        r#"
tracker.list_issues:
  cache: { key: [query], ttlS: 900 }
  paginate: { param: cursor, deliver: first_page, backgroundFetch: all }
  shape: { strip: [body], toFile: true }
tracker.get_issue:
  serveFrom: { fetchedBy: tracker.list_issues, match: id }
  onMiss: fetch_live
tracker.summarize:
  cache: { key: [query], ttlS: 900 }
  shape: { strip: [body], toFile: true }
"#,
    )
    .unwrap();
    let mut gateway = McpGateway::new(
        vec![ServerConfig::for_local_test("tracker", server.url.clone())],
        policies,
        dirs.path().join("cache"),
        dirs.path().join("artifacts"),
        Some(sink),
    )
    .unwrap();
    let list = ToolCall {
        server: "tracker".into(),
        tool: "list_issues".into(),
        args: json!({"query":"test"}),
    };
    let first = gateway.call(&list).unwrap();
    gateway.call(&list).unwrap();
    let worklist = first
        .artifacts
        .iter()
        .find(|path| {
            path.extension()
                .is_some_and(|extension| extension == "jsonl")
        })
        .unwrap();
    let complete_items: Vec<Value> = std::fs::read_to_string(worklist)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let complete_bytes = bytes(&json!(complete_items));
    gateway
        .call(&ToolCall {
            server: "tracker".into(),
            tool: "get_issue".into(),
            args: json!({"id":1}),
        })
        .unwrap();
    let raw = json!({"body":"private-\u{00e9}-\u{1f980}".repeat(100), "answer": 42});
    server
        .tool_payloads
        .lock()
        .unwrap()
        .insert("summarize".into(), raw.clone());
    let simple = ToolCall {
        server: "tracker".into(),
        tool: "summarize".into(),
        args: json!({"query":"x"}),
    };
    let shaped = gateway.call(&simple).unwrap();
    gateway.call(&simple).unwrap();
    let local = ToolCall {
        server: "local".into(),
        tool: "calculate".into(),
        args: json!({"operation":"add", "left":"2", "right":"3"}),
    };
    let calculated = gateway.call(&local).unwrap();
    let mut bad = local.clone();
    bad.args["operation"] = json!("divide");
    bad.args["right"] = json!("0");
    assert!(gateway.call(&bad).is_err());
    assert!(gateway
        .call(&ToolCall {
            server: "tracker".into(),
            tool: "get_issue".into(),
            args: json!({"id":999, "toolError":"structured"})
        })
        .is_err());
    assert_eq!(server.count("call:list_issues"), 3);
    let events = events.lock().unwrap();
    let sizes: Vec<_> = events
        .iter()
        .filter(|e| e["type"] == "response_size")
        .collect();
    let find = |id, stage: &str| {
        *sizes
            .iter()
            .find(|e| e["dispatch_id"] == id && e["stage"] == stage)
            .unwrap()
    };
    assert_eq!(
        find(1, "gateway_result")["compact_json_utf8_bytes"],
        bytes(&first.content_for_model)
    );
    assert_eq!(
        sizes
            .iter()
            .filter(|e| e["dispatch_id"] == 1 && e["stage"] == "raw_page")
            .count(),
        3
    );
    assert!(
        find(1, "complete_collection")["compact_json_utf8_bytes"]
            .as_u64()
            .unwrap()
            > find(1, "selected_page")["compact_json_utf8_bytes"]
                .as_u64()
                .unwrap()
    );
    for id in [1, 2] {
        assert_eq!(
            find(id, "complete_collection")["compact_json_utf8_bytes"],
            complete_bytes
        );
    }
    assert_eq!(find(2, "gateway_result")["source"], "cache");
    assert_eq!(find(3, "raw_payload")["source"], "served_from_file");
    assert_eq!(
        find(4, "raw_payload")["compact_json_utf8_bytes"],
        bytes(&raw)
    );
    assert_eq!(
        find(4, "gateway_result")["compact_json_utf8_bytes"],
        bytes(&shaped.content_for_model)
    );
    assert!(bytes(&raw) > bytes(&shaped.content_for_model));
    assert_eq!(find(5, "raw_payload")["source"], "cache");
    assert_eq!(find(6, "raw_payload")["source"], "local");
    assert_eq!(
        find(6, "gateway_result")["compact_json_utf8_bytes"],
        bytes(&calculated.content_for_model)
    );
    assert!(find(7, "failed_result")["compact_json_utf8_bytes"].is_null());
    assert_eq!(
        find(8, "raw_error_payload")["compact_json_utf8_bytes"],
        bytes(&json!({"failure":true}))
    );
    assert!(find(8, "failed_result")["compact_json_utf8_bytes"].is_null());
    assert!(!serde_json::to_string(&sizes).unwrap().contains("private-"));
}

#[test]
fn discovery_failure_is_redacted_and_stays_before_tool_dispatch() {
    let remote_error = json!({"code":-32603,"message":"private-diagnostic","data":"private-data"});
    let server = MockMcpServer::start_with_tools_response(json!({"error":remote_error}));
    let dirs = TempDir::new("discovery-error-size");
    let events = Arc::new(Mutex::new(Vec::<Value>::new()));
    let captured = events.clone();
    let sink: EventSink = Box::new(move |e| {
        captured.lock().unwrap().push(serde_json::to_value(e)?);
        Ok(())
    });
    let mut gateway = McpGateway::new(
        vec![ServerConfig::for_local_test("tracker", server.url.clone())],
        PolicySet::default(),
        dirs.path().join("cache"),
        dirs.path().join("artifacts"),
        Some(sink),
    )
    .unwrap();
    let error = gateway
        .call(&ToolCall {
            server: "tracker".into(),
            tool: "get_issue".into(),
            args: json!({"id":1}),
        })
        .unwrap_err();
    assert!(!format!("{error:#}").contains("private-"));
    let events = events.lock().unwrap();
    assert!(events.is_empty());
    assert!(!serde_json::to_string(&*events)
        .unwrap()
        .contains("private-"));
    assert_eq!(server.count("call:get_issue"), 0);
}

#[test]
fn empty_collection_is_two_compact_array_bytes_live_and_cached() {
    let server = MockMcpServer::start();
    server
        .tool_payloads
        .lock()
        .unwrap()
        .insert("list_issues".into(), json!({"items":[], "cursor":null}));
    let dirs = TempDir::new("empty-response-sizes");
    let sizes = Arc::new(Mutex::new(Vec::<Value>::new()));
    let captured = sizes.clone();
    let mut gateway = McpGateway::new(
        vec![ServerConfig::for_local_test("tracker", server.url.clone())],
        PolicySet::from_yaml_str("tracker.list_issues: {cache: {key: [query], ttlS: 900}, paginate: {param: cursor, deliver: first_page, backgroundFetch: all}}").unwrap(),
        dirs.path().join("cache"), dirs.path().join("artifacts"),
        Some(Box::new(move |event| { captured.lock().unwrap().push(serde_json::to_value(event)?); Ok(()) })),
    ).unwrap();
    let call = ToolCall {
        server: "tracker".into(),
        tool: "list_issues".into(),
        args: json!({"query":"empty"}),
    };
    gateway.call(&call).unwrap();
    gateway.call(&call).unwrap();
    let sizes = sizes.lock().unwrap();
    let collections: Vec<_> = sizes
        .iter()
        .filter(|event| event["stage"] == "complete_collection")
        .collect();
    assert_eq!(collections.len(), 2);
    for collection in collections {
        assert_eq!(collection["compact_json_utf8_bytes"], 2);
    }
    assert_eq!(server.count("call:list_issues"), 1);
}

#[test]
fn exact_call_context_and_entered_timings_survive_repeated_dispatches_and_failure() {
    let server = MockMcpServer::start();
    let dirs = TempDir::new("correlated-response-size");
    let events = Arc::new(Mutex::new(Vec::<Value>::new()));
    let captured = events.clone();
    let sink: EventSink = Box::new(move |event| {
        captured.lock().unwrap().push(serde_json::to_value(event)?);
        Ok(())
    });
    let mut gateway = McpGateway::new(
        vec![ServerConfig::for_local_test("tracker", server.url.clone())],
        PolicySet::from_yaml_str("tracker.get_issue: {cache: {key: [id], ttlS: 900}}").unwrap(),
        dirs.path().join("cache"),
        dirs.path().join("artifacts"),
        Some(sink),
    )
    .unwrap();
    let call = ToolCall {
        server: "tracker".into(),
        tool: "get_issue".into(),
        args: json!({"id":1}),
    };
    for (epoch, attempt, ordinal) in [("epoch-a", 1, 1), ("epoch-a", 1, 2), ("epoch-b", 1, 1)] {
        gateway.set_call_context(Some(trollm_mcp::CallContext {
            epoch: epoch.into(),
            attempt,
            call: ordinal,
        }));
        gateway.call(&call).unwrap();
    }
    gateway.set_call_context(Some(trollm_mcp::CallContext {
        epoch: "epoch-b".into(),
        attempt: 2,
        call: 1,
    }));
    let failed = ToolCall {
        server: "tracker".into(),
        tool: "get_issue".into(),
        args: json!({"id":999,"toolError":"structured"}),
    };
    assert!(gateway.call(&failed).is_err());
    gateway.set_call_context(None);
    let local = ToolCall {
        server: "local".into(),
        tool: "calculate".into(),
        args: json!({"operation":"add","left":"2","right":"3"}),
    };
    gateway.call(&local).unwrap();
    let events = events.lock().unwrap();
    let final_events: Vec<_> = events
        .iter()
        .filter(|event| event["stage"] == "gateway_result" || event["stage"] == "failed_result")
        .collect();
    assert_eq!(final_events.len(), 5);
    for (index, expected) in [
        json!({"epoch":"epoch-a","attempt":1,"call":1}),
        json!({"epoch":"epoch-a","attempt":1,"call":2}),
        json!({"epoch":"epoch-b","attempt":1,"call":1}),
        json!({"epoch":"epoch-b","attempt":2,"call":1}),
    ]
    .iter()
    .enumerate()
    {
        let final_event = final_events[index];
        assert_eq!(&final_event["call_context"], expected);
        assert!(final_event["timings"]["elapsedMs"].is_u64());
        let dispatch = final_event["dispatch_id"].clone();
        assert!(events
            .iter()
            .filter(|event| event["type"] == "response_size" && event["dispatch_id"] == dispatch)
            .all(|event| &event["call_context"] == expected));
    }
    let first = final_events[0];
    assert!(first["timings"]["transportMs"].is_u64());
    assert!(first["timings"]["resultFinishMs"].is_u64());
    assert!(first["timings"]["resultAdmissionMs"].is_u64());
    // Cache observations have no new transport measurement; zero is not used
    // as a substitute for a phase that was never entered.
    assert!(final_events[1]["timings"]["transportMs"].is_null());
    assert!(final_events[1]["timings"]["cacheReadMs"].is_u64());
    let failed = final_events[3];
    assert_eq!(failed["stage"], "failed_result");
    assert!(failed["compact_json_utf8_bytes"].is_null());
    assert!(failed["timings"]["transportMs"].is_u64());
    assert!(failed["timings"]["resultFinishMs"].is_null());
    assert!(final_events[4]["call_context"].is_null());
    assert!(final_events[4]["timings"]["transportMs"].is_null());
}

#[test]
fn cached_result_validation_times_pre_cache_admission_failure_and_final_processing() {
    let server = MockMcpServer::start();
    let dirs = TempDir::new("cached-admission-timing");
    let events = Arc::new(Mutex::new(Vec::<Value>::new()));
    let captured = events.clone();
    let sink: EventSink = Box::new(move |event| {
        captured.lock().unwrap().push(serde_json::to_value(event)?);
        Ok(())
    });
    let policies = PolicySet::from_yaml_str(
        &json!({
            "tracker.get_issue": {
                "cache":{"key":["id"],"ttlS":900},
                "resultArtifact":"validated",
                "resultValidation":{"maxBytes":2048,"schema":{
                    "type":"object","required":["id"],
                    "properties":{"id":{"type":"integer"}}
                }}
            }
        })
        .to_string(),
    )
    .unwrap();
    let mut gateway = McpGateway::new(
        vec![ServerConfig::for_local_test("tracker", server.url.clone())],
        policies,
        dirs.path().join("cache"),
        dirs.path().join("artifacts"),
        Some(sink),
    )
    .unwrap();
    let call = ToolCall {
        server: "tracker".into(),
        tool: "get_issue".into(),
        args: json!({"id":1}),
    };
    server
        .tool_payloads
        .lock()
        .unwrap()
        .insert("get_issue".into(), json!({"id":"invalid"}));
    assert!(gateway.call(&call).is_err());
    let meta = dirs.path().join("artifacts/validated.json.meta.json");
    let failure_meta: Value = serde_json::from_slice(&std::fs::read(&meta).unwrap()).unwrap();
    assert_eq!(failure_meta["complete"], false);
    assert!(!dirs.path().join("artifacts/validated.json").exists());
    {
        let events = events.lock().unwrap();
        let failed = events
            .iter()
            .find(|event| event["stage"] == "failed_result")
            .unwrap();
        assert!(failed["timings"]["resultAdmissionMs"].is_u64());
        assert!(failed["timings"]["resultFinishMs"].is_null());
        assert!(failed["timings"]["transportMs"].is_u64());
        assert!(failed["compact_json_utf8_bytes"].is_null());
    }
    // The rejected payload never filled the cache. The same arguments must
    // fetch again, validate before filling, then validate within finish_result.
    server
        .tool_payloads
        .lock()
        .unwrap()
        .insert("get_issue".into(), json!({"id":1}));
    let live = gateway.call(&call).unwrap();
    assert_eq!(live.content_for_model["id"], 1);
    assert_eq!(server.count("call:get_issue"), 2);
    let completed_meta: Value = serde_json::from_slice(&std::fs::read(&meta).unwrap()).unwrap();
    assert_eq!(completed_meta["complete"], true);
    let cached = gateway.call(&call).unwrap();
    assert_eq!(cached.content_for_model, live.content_for_model);
    assert_eq!(server.count("call:get_issue"), 2);
    let events = events.lock().unwrap();
    let finals: Vec<_> = events
        .iter()
        .filter(|event| event["stage"] == "gateway_result")
        .collect();
    assert_eq!(finals.len(), 2);
    for event in &finals {
        assert!(event["timings"]["resultAdmissionMs"].is_u64());
        assert!(event["timings"]["resultFinishMs"].is_u64());
        assert!(event["timings"]["elapsedMs"].is_u64());
    }
    assert!(finals[0]["timings"]["transportMs"].is_u64());
    assert!(finals[1]["timings"]["transportMs"].is_null());
    assert!(finals[1]["timings"]["cacheReadMs"].is_u64());
    // Millisecond rounding cannot establish containment or invocation counts;
    // no subtraction/comparison pretends admission is wholly inside finish.
}
