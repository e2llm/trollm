//! Literal parsed-JSON equality using the ordinary local gateway.
#[allow(dead_code, unused_imports)]
mod common;

use common::TempDir;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use trollm_mcp::{local, EventSink, McpGateway, PolicySet, Provenance, ToolCall, ToolGateway};

fn args(left: Value, right: Value) -> Value {
    json!({"left":left,"right":right})
}

#[test]
fn equal_values_preserve_structural_types_object_keys_and_array_order() {
    for value in [
        Value::Null,
        json!(false),
        json!(17),
        json!(1.0),
        json!("#literal"),
        json!([1, null, true]),
        json!({"id":"canonical","tag":"div","attributes":{"id":"literal"}}),
    ] {
        assert_eq!(
            local::assert_equal(&args(value.clone(), value)).unwrap(),
            json!({"equal":true})
        );
    }
    let left: Value = serde_json::from_str(r#"{"b":2,"a":{"z":null,"x":[1,2]}}"#).unwrap();
    let right: Value = serde_json::from_str(r#"{"a":{"x":[1,2],"z":null},"b":2}"#).unwrap();
    assert_eq!(
        local::assert_equal(&args(left, right)).unwrap(),
        json!({"equal":true})
    );
}

#[test]
fn mismatches_are_literal_and_errors_do_not_echo_either_operand() {
    for (left, right) in [
        (json!("PRIVATE-left"), json!("private-left")),
        (json!("é"), json!("e\u{0301}")),
        (json!("%23scope"), json!("#scope")),
        (json!([1, 2]), json!([2, 1])),
        (json!({"a":null}), json!({})),
        (json!(true), json!(1)),
        (json!("1"), json!(1)),
    ] {
        assert_eq!(
            local::assert_equal(&args(left, right))
                .unwrap_err()
                .to_string(),
            "local.assert_equal: mismatch"
        );
    }
}

#[test]
fn parsed_serde_numbers_have_explicit_representation_semantics() {
    for (left, right, equal) in [
        ("1", "1.0", false),
        ("-1", "-1.0", false),
        ("1.0", "1e0", true),
        ("0.0", "-0.0", true),
        ("0", "0.0", false),
    ] {
        let input = args(
            serde_json::from_str(left).unwrap(),
            serde_json::from_str(right).unwrap(),
        );
        assert_eq!(
            local::assert_equal(&input).is_ok(),
            equal,
            "{left} versus {right}"
        );
    }
}

#[test]
fn closed_arguments_and_standard_combined_byte_depth_limits_fail_without_truncation() {
    for input in [
        Value::Null,
        json!({}),
        json!({"left":null}),
        json!({"right":null}),
        json!({"left":1,"right":1,"extra":true}),
    ] {
        assert_eq!(
            local::assert_equal(&input).unwrap_err().to_string(),
            "local.assert_equal: invalid-arguments"
        );
    }
    let large = json!("x".repeat(140_000));
    assert!(local::assert_equal(&args(large.clone(), large))
        .unwrap_err()
        .to_string()
        .contains("serialized size limit"));
    let mut deep = json!(null);
    for _ in 0..65 {
        deep = json!([deep]);
    }
    assert!(local::assert_equal(&args(deep.clone(), deep))
        .unwrap_err()
        .to_string()
        .contains("nesting limit"));
}

#[test]
fn gateway_uses_catalog_constraints_artifact_policy_and_numeric_only_telemetry() {
    let dirs = TempDir::new("assert-equal-gateway");
    let events = Arc::new(Mutex::new(Vec::<Value>::new()));
    let sink_events = events.clone();
    let sink: EventSink = Box::new(move |event| {
        sink_events
            .lock()
            .unwrap()
            .push(serde_json::to_value(event)?);
        Ok(())
    });
    let policies=PolicySet::from_yaml_str(&json!({"local.assert_equal":{"resultArtifact":"equality-proof", "resultValidation":{"maxBytes":128,
        "schema":serde_json::from_str::<Value>(include_str!("../../../schemas/local-assert-equal.v1.output.schema.json")).unwrap()}}}).to_string()).unwrap();
    let mut gateway = McpGateway::new(
        vec![],
        policies,
        dirs.path().join("cache"),
        dirs.path().join("artifacts"),
        Some(sink),
    )
    .unwrap()
    .with_argument_constraints(BTreeMap::from([(
        "local.assert_equal".into(),
        json!({"properties":{"left":{"type":"object"},"right":{"type":"object"}}}),
    )]))
    .unwrap();
    assert!(gateway
        .list_tools()
        .unwrap()
        .iter()
        .any(|tool| tool.server == "local" && tool.name == "assert_equal"));
    let identity = json!({"id":"PRIVATE-canonical","tag":"div","attributes":{"id":"literal"}});
    let call = ToolCall {
        server: "local".into(),
        tool: "assert_equal".into(),
        args: args(identity.clone(), identity),
    };
    let result = gateway.call(&call).unwrap();
    assert_eq!(result.provenance, Provenance::Local);
    assert_eq!(result.content_for_model, json!({"equal":true}));
    let saved: Value = serde_json::from_slice(
        &std::fs::read(dirs.path().join("artifacts/equality-proof.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(saved, json!({"equal":true}));
    let events = events.lock().unwrap();
    assert!(!serde_json::to_string(&*events)
        .unwrap()
        .contains("PRIVATE-canonical"));
    for stage in ["raw_payload", "gateway_result"] {
        assert!(events.iter().any(|event| event["type"] == "response_size"
            && event["stage"] == stage
            && event["compact_json_utf8_bytes"] == 14));
    }
    assert!(events.iter().any(|event| event["type"] == "local_call"
        && event["args_hash"]
            .as_str()
            .is_some_and(|hash| hash.len() == 64)));
    drop(events);
    let invalid = ToolCall {
        args: args(json!(1), json!(1)),
        ..call.clone()
    };
    assert!(gateway.validate_call(&invalid).is_err());
    let oversized = ToolCall {
        args: args(
            json!({"x":"x".repeat(140_000)}),
            json!({"x":"x".repeat(140_000)}),
        ),
        ..call
    };
    assert!(gateway
        .validate_call(&oversized)
        .unwrap_err()
        .to_string()
        .contains("serialized size limit"));
}

#[test]
fn mismatch_publishes_no_success_artifact_and_read_tool_rejects_mutation_or_cache_policy() {
    let dirs = TempDir::new("assert-equal-failure");
    let policies =
        PolicySet::from_yaml_str("local.assert_equal: {resultArtifact: equality-proof}").unwrap();
    let mut gateway = McpGateway::new(
        vec![],
        policies,
        dirs.path().join("cache"),
        dirs.path().join("artifacts"),
        None,
    )
    .unwrap();
    let error = gateway
        .call(&ToolCall {
            server: "local".into(),
            tool: "assert_equal".into(),
            args: args(json!("PRIVATE-a"), json!("PRIVATE-b")),
        })
        .unwrap_err();
    assert_eq!(error.to_string(), "local.assert_equal: mismatch");
    assert!(!dirs.path().join("artifacts/equality-proof.json").exists());
    for policy in [json!({"class":"mutation"}), json!({"cache":{"ttlS":30}})] {
        let policies =
            PolicySet::from_yaml_str(&json!({"local.assert_equal":policy}).to_string()).unwrap();
        assert!(McpGateway::new(
            vec![],
            policies,
            dirs.path().join("cache"),
            dirs.path().join("artifacts"),
            None
        )
        .is_err());
    }
}
