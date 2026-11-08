use serde_json::{json, Value};
use trollm_mcp::local;

#[allow(dead_code, unused_imports)]
mod common;

fn invoke(tool: &str, items: Value, schema: Value) -> Value {
    local::invoke(tool, &json!({"items":items,"schema":schema})).unwrap()
}

#[test]
fn filtering_preserves_original_objects_order_indices_and_exclusion_counts() {
    let original = json!({"id":"counter","text":"1","selector":"div.cart span.quantity","extra":{"keep":[1,2]}});
    let items = json!([{"id":"price","text":"6.97"},original,{"id":"missing"}]);
    let predicate = json!({"type":"object","required":["text"],"properties":{"text":{"type":"string","pattern":"^[0-9]+$"}}});
    let result = invoke("filter_records", items.clone(), predicate);
    assert_eq!(result["matches"], json!([{"index":1,"selected":original}]));
    assert_eq!(result["matchedCount"], 1);
    assert_eq!(result["excludedCount"], 2);
    assert_eq!(result["sourceSha256"].as_str().unwrap().len(), 64);
    let all = invoke("filter_records", items, json!({"type":"object"}));
    assert_eq!(all["sourceSha256"], result["sourceSha256"]);
}

#[test]
fn unique_structural_match_refuses_absence_and_duplicates() {
    let predicate =
        json!({"type":"object","required":["tag"],"properties":{"tag":{"const":"form"}}});
    let result = invoke(
        "select_matching",
        json!([{"tag":"input"},{"tag":"form","id":"original"}]),
        predicate.clone(),
    );
    assert_eq!(result["selected"]["id"], "original");
    for (items, suffix) in [
        (json!([]), "no-match"),
        (json!([{"tag":"form"},{"tag":"form"}]), "ambiguous"),
    ] {
        assert_eq!(
            local::invoke(
                "select_matching",
                &json!({"items":items,"schema":predicate})
            )
            .unwrap_err()
            .to_string(),
            format!("local.select_matching: {suffix}")
        );
    }
}

fn ordered(items: Value) -> Value {
    json!({"items":items,"schema":{"type":"object","required":["text"]},
        "field":"/text","prefix":"$","fractionDigits":2,"groupSize":2})
}

#[test]
fn decimal_ordering_is_numeric_stable_and_retains_every_admitted_record() {
    let input = ordered(
        json!([{"id":"a","text":"$10.00"},{"id":"b","text":"$2.00"},{"id":"c","text":"$2.00"},{"other":true}]),
    );
    let result = local::invoke("order_records", &input).unwrap();
    assert_eq!(result["groups"][0]["items"][0]["selected"]["id"], "b");
    assert_eq!(result["groups"][0]["items"][1]["selected"]["id"], "c");
    assert_eq!(result["groups"][1]["items"][0]["selected"]["id"], "a");
    assert_eq!(result["groups"][0]["items"][0]["decimal"], "2");
    assert_eq!(result["matchedCount"], 3);
    assert_eq!(result["excludedCount"], 1);
    for label in [
        "$2.001",
        "$2.00 per unit",
        "$2.00$9.00",
        "$1,200.00",
        "€2.00",
        "$NaN",
    ] {
        assert!(
            local::invoke("order_records", &ordered(json!([{"text":label}]))).is_err(),
            "{label}"
        );
    }
}

#[test]
fn invalid_predicates_references_records_and_limits_fail_without_partial_results() {
    for input in [
        json!({"items":[17],"schema":{}}),
        json!({"items":[],"schema":true}),
        json!({"items":[],"schema":{"$ref":"https://example.invalid/schema"}}),
        json!({"items":[],"schema":{"type":"unknown"}}),
        json!({"items":[],"schema":{},"extra":true}),
        json!({"items":vec![json!({});10_001],"schema":{}}),
    ] {
        assert!(local::invoke("filter_records", &input).is_err());
    }
    let output = invoke("filter_records", json!([]), json!({"type":"object"}));
    assert_eq!(output["matches"], json!([]));
    for pointer in ["field", "/bad~2"] {
        let mut input = ordered(json!([]));
        input["field"] = json!(pointer);
        assert!(local::invoke("order_records", &input).is_err());
    }
}

#[test]
fn published_schemas_accept_actual_results_and_reject_missing_arguments() {
    for (tool, name, input) in [
        (
            "filter_records",
            "filter-records",
            json!({"items":[{"tag":"form"}],"schema":{}}),
        ),
        (
            "select_matching",
            "select-matching",
            json!({"items":[{"tag":"form"}],"schema":{}}),
        ),
        (
            "order_records",
            "order-records",
            ordered(json!([{"text":"$1.25"}])),
        ),
    ] {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../schemas");
        for (kind, value) in [
            ("input", input.clone()),
            ("output", local::invoke(tool, &input).unwrap()),
        ] {
            let schema: Value = serde_json::from_slice(
                &std::fs::read(root.join(format!("local-{name}.v1.{kind}.schema.json"))).unwrap(),
            )
            .unwrap();
            let validator = jsonschema::validator_for(&schema).unwrap();
            assert!(validator.is_valid(&value), "{tool} {kind}");
            if kind == "input" {
                assert!(!validator.is_valid(&json!({})));
            }
        }
    }
}

#[test]
fn real_gateway_validates_large_bound_sources_and_publishes_complete_artifacts() {
    use trollm_mcp::{McpGateway, PolicySet, Provenance, ToolCall, ToolGateway};
    let dir = common::TempDir::new("record-tool-gateway");
    let policy =
        PolicySet::from_yaml_str("local.filter_records:\n  resultArtifact: records\n").unwrap();
    let mut gateway = McpGateway::new(
        vec![],
        policy,
        dir.path().join("cache"),
        dir.path().join("artifacts"),
        None,
    )
    .unwrap();
    let call = ToolCall {
        server: "local".into(),
        tool: "filter_records".into(),
        args: json!({
            "items":[{"id":"large","body":"x".repeat(300_000)},{"id":"kept","tag":"form"}],
            "schema":{"type":"object","required":["tag"],"properties":{"tag":{"const":"form"}}}
        }),
    };
    gateway.validate_call(&call).unwrap();
    let result = gateway.call(&call).unwrap();
    assert_eq!(result.provenance, Provenance::Local);
    assert_eq!(
        result.content_for_model["matches"][0]["selected"]["id"],
        "kept"
    );
    let saved: Value =
        serde_json::from_slice(&std::fs::read(dir.path().join("artifacts/records.json")).unwrap())
            .unwrap();
    assert_eq!(saved, result.content_for_model);
    let mut invalid = call;
    invalid.args["schema"] = json!({"$ref":"https://example.invalid/no-network"});
    assert!(gateway.validate_call(&invalid).is_err());
}
