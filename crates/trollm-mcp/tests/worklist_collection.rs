//! Generic bounded collection tools; fixtures require no models or network.
#[allow(dead_code, unused_imports)]
mod common;

use common::TempDir;
use serde_json::{json, Value};
use trollm_mcp::{local, McpGateway, PolicySet, Provenance, ToolCall, ToolGateway};

fn prepared(envelope: Value) -> Value {
    json!({"input":serde_json::to_string(&envelope).unwrap()})
}
fn total() -> Value {
    json!({"expected":[{"key":"ticket-a"},{"key":"ticket-b"}],
        "results":[{"key":"ticket-b","state":"succeeded","cost":"-0.10"},
                   {"key":"ticket-a","state":"succeeded","cost":"1.200000"}],
        "expectedId":"/key","resultId":"/key","outcome":"/state","amount":"/cost"})
}

#[test]
fn freezes_original_generic_values_and_distinguishes_occurrences() {
    let entry = json!({"task":"Review incident α", "unknown":[true,null,{"hours":"0.50"}]});
    let args = prepared(json!({"context":"On-call handoff", "items":[entry,entry]}));
    let result = local::prepare_worklist(&args).unwrap();
    assert_eq!(result, local::prepare_worklist(&args).unwrap());
    assert_eq!(result["context"], "On-call handoff");
    assert_eq!(result["items"][0]["value"], entry);
    assert_eq!(result["items"][1]["value"], entry);
    assert_ne!(result["items"][0]["id"], result["items"][1]["id"]);
    assert_eq!(result["items"][0]["id"].as_str().unwrap().len(), 64);
    let changed = prepared(json!({"context":"Other handoff", "items":[entry]}));
    assert_ne!(
        result["items"][0]["id"],
        local::prepare_worklist(&changed).unwrap()["items"][0]["id"]
    );
    assert!(local::prepare_worklist(&prepared(json!({"context":"","items":[{}]}))).is_ok());
}

#[test]
fn worklist_rejects_malformed_empty_unknown_and_oversized_inputs() {
    for input in [
        Value::Null,
        json!({}),
        json!({"input":""}),
        json!({"input":"private-not-json"}),
        json!({"input":"{}","extra":true}),
        prepared(json!({"context":"","items":[]})),
        prepared(json!({"context":"","items":[null]})),
        prepared(json!({"context":"","items":[{}],"extra":true})),
        prepared(json!({"items":[{}]})),
        prepared(json!({"context":1,"items":[{}]})),
        prepared(json!({"context":"","items":vec![json!({});9]})),
        prepared(json!({"context":"é".repeat(2049),"items":[{}]})),
        prepared(json!({"context":"","items":[{"text":"é".repeat(2048)}]})),
        json!({"input": " ".repeat(48*1024+1)}),
    ] {
        let error = local::prepare_worklist(&input).unwrap_err().to_string();
        assert!(!error.contains("private-not-json"));
    }
    let nested = (0..33).fold(Value::Null, |inner, _| json!([inner]));
    assert!(
        local::prepare_worklist(&prepared(json!({"context":"","items":[{"deep":nested}]})))
            .is_err()
    );
    assert!(
        local::prepare_worklist(&prepared(json!({"context":"é".repeat(2048),"items":[{}]})))
            .is_ok()
    );
}

#[test]
fn total_is_exact_complete_ordered_and_retains_original_amount_spellings() {
    assert_eq!(
        local::collection_total(&total()).unwrap(),
        json!({
        "format":"trollm.local.collection_total.v1","count":2,"sum":"1.1",
        "terms":[{"id":"ticket-a","amount":"1.200000"},{"id":"ticket-b","amount":"-0.10"}]})
    );
    let mut escaped = total();
    escaped["expected"] = json!([{"a/b":{"~":"ticket-a"}},{"a/b":{"~":"ticket-b"}}]);
    escaped["expectedId"] = json!("/a~1b/~0");
    assert!(local::collection_total(&escaped).is_ok());
}

#[test]
fn total_rejects_duplicate_missing_unexpected_failed_and_skipped_records() {
    for (field, value) in [
        ("expected", json!([{"key":"ticket-a"},{"key":"ticket-a"}])),
        (
            "results",
            json!([{"key":"ticket-a","state":"succeeded","cost":"1"},{"key":"ticket-a","state":"succeeded","cost":"1"}]),
        ),
        (
            "results",
            json!([{"key":"ticket-a","state":"succeeded","cost":"1"}]),
        ),
        (
            "results",
            json!([{"key":"other","state":"succeeded","cost":"1"}]),
        ),
        ("expected", json!([])),
        ("results", json!([])),
        ("expected", json!([{"key":null}])),
    ] {
        let mut args = total();
        args[field] = value;
        assert!(local::collection_total(&args).is_err(), "{field}");
    }
    for outcome in [json!("failed"), json!("skipped"), json!(null), json!(true)] {
        let mut args = total();
        args["results"][0]["state"] = outcome;
        assert!(local::collection_total(&args).is_err());
    }
}

#[test]
fn total_rejects_invalid_decimal_pointer_shape_bytes_depth_and_overflow() {
    for amount in ["0.0000001", "1e2", "01", "1000000000000", "NaN", ""] {
        let mut args = total();
        args["results"][0]["cost"] = json!(amount);
        assert!(local::collection_total(&args).is_err());
    }
    let mut overflow = total();
    for index in 0..2 {
        overflow["results"][index]["cost"] = json!("999999999999.999999");
    }
    assert_eq!(
        local::collection_total(&overflow).unwrap_err().to_string(),
        "local.collection_total: out-of-range"
    );
    for pointer in ["#fragment", "key", "/~2", "/~"] {
        let mut args = total();
        args["expectedId"] = json!(pointer);
        assert!(local::collection_total(&args).is_err());
    }
    for value in [json!("é".repeat(2049)), json!("x".repeat(4097))] {
        let mut args = total();
        args["expectedId"] = value;
        assert!(local::collection_total(&args).is_err());
    }
    let mut args = total();
    args["extra"] = json!(null);
    assert!(local::collection_total(&args).is_err());
    let mut args = total();
    args["results"][0]["evidence"] = json!("x".repeat(256 * 1024));
    assert!(local::collection_total(&args).is_err());
    let mut args = total();
    args["results"][0]["evidence"] = (0..65).fold(Value::Null, |inner, _| json!([inner]));
    assert!(local::collection_total(&args).is_err());
}

#[test]
fn gateway_catalog_admission_result_schemas_and_original_artifacts() {
    let dirs = TempDir::new("worklist-collection");
    let mut policies = serde_json::Map::new();
    for (tool, artifact, schema) in [
        (
            "prepare_worklist",
            "worklist",
            include_str!("../../../schemas/local-prepare-worklist.v1.output.schema.json"),
        ),
        (
            "collection_total",
            "total",
            include_str!("../../../schemas/local-collection-total.v1.output.schema.json"),
        ),
    ] {
        policies.insert(format!("local.{tool}"),json!({"resultArtifact":artifact,"resultValidation":{"maxBytes":65536,"schema":serde_json::from_str::<Value>(schema).unwrap()}}));
    }
    let mut gateway = McpGateway::new(
        vec![],
        PolicySet::from_yaml_str(&Value::Object(policies).to_string()).unwrap(),
        dirs.path().join("cache"),
        dirs.path().join("artifacts"),
        None,
    )
    .unwrap();
    for (tool, args, artifact) in [
        (
            "prepare_worklist",
            prepared(json!({"context":"Tickets","items":[{"issue":"network"}]})),
            "worklist",
        ),
        ("collection_total", total(), "total"),
    ] {
        assert!(gateway
            .list_tools()
            .unwrap()
            .iter()
            .any(|entry| entry.server == "local" && entry.name == tool));
        let result = gateway
            .call(&ToolCall {
                server: "local".into(),
                tool: tool.into(),
                args,
            })
            .unwrap();
        assert_eq!(result.provenance, Provenance::Local);
        let saved: Value = serde_json::from_slice(
            &std::fs::read(dirs.path().join(format!("artifacts/{artifact}.json"))).unwrap(),
        )
        .unwrap();
        assert_eq!(saved, result.content_for_model);
    }
    let bad = prepared(json!({"context":"é".repeat(2049),"items":[{}]}));
    assert!(gateway
        .validate_call(&ToolCall {
            server: "local".into(),
            tool: "prepare_worklist".into(),
            args: bad
        })
        .is_err());
    let mut bad = total();
    bad["amount"] = json!("/é".repeat(2049));
    assert!(gateway
        .validate_call(&ToolCall {
            server: "local".into(),
            tool: "collection_total".into(),
            args: bad
        })
        .is_err());
}

#[test]
fn boundary_counts_item_bytes_and_root_identity_pointer_are_supported() {
    let item = json!({"text":"x".repeat(4085)}); // 11 JSON overhead bytes, exactly 4096.
    assert_eq!(serde_json::to_vec(&item).unwrap().len(), 4096);
    let args = prepared(json!({"context":"","items":vec![item;8]}));
    assert_eq!(
        local::prepare_worklist(&args).unwrap()["items"]
            .as_array()
            .unwrap()
            .len(),
        8
    );
    let args = json!({"expected":["incident"],"results":[{"id":"incident","status":"succeeded","value":"-0.000000"}],
        "expectedId":"","resultId":"/id","outcome":"/status","amount":"/value"});
    let result = local::collection_total(&args).unwrap();
    assert_eq!(result["sum"], "0");
    assert_eq!(result["terms"][0]["amount"], "-0.000000");
}

#[test]
fn generic_count_matrix_and_closed_collection_argument_shapes() {
    for count in [1, 2, 3, 8] {
        let items = (0..count)
            .map(|index| json!({"incident":format!("case {index}"),"estimate":"0.125"}))
            .collect::<Vec<_>>();
        let prepared = local::prepare_worklist(&prepared(
            json!({"context":"engineering hours","items":items}),
        ))
        .unwrap();
        assert_eq!(prepared["items"].as_array().unwrap().len(), count);
        let results=prepared["items"].as_array().unwrap().iter().map(|item|json!({"id":item["id"],"state":"succeeded","hours":item["value"]["estimate"]})).collect::<Vec<_>>();
        let args = json!({"expected":prepared["items"],"results":results,"expectedId":"/id","resultId":"/id","outcome":"/state","amount":"/hours"});
        let sum = local::collection_total(&args).unwrap();
        assert_eq!(sum["count"], count);
        assert_eq!(
            sum["sum"],
            match count {
                1 => "0.125",
                2 => "0.25",
                3 => "0.375",
                8 => "1",
                _ => unreachable!(),
            }
        );
    }
    for invalid in [Value::Null, json!([]), json!({})] {
        assert!(local::collection_total(&invalid).is_err());
    }
    for field in ["expected", "results"] {
        let mut args = total();
        args[field] = json!(vec![json!({}); 9]);
        assert!(local::collection_total(&args).is_err());
    }
    let mut args = total();
    args["results"][0]["cost"] = json!(1.0);
    assert!(local::collection_total(&args).is_err());
}

#[test]
fn independently_bound_finite_roster_refuses_changed_subset_empty_and_reorder() {
    // The selected original is a trusted installation input, not model output.
    // This exercises the existing gateway constraint before any snapshot/tool.
    for count in 1..=3 {
        let items = (0..count)
            .map(|index| {
                json!({"id":format!("item-{index}"),
            "quantity":index+1})
            })
            .collect::<Vec<_>>();
        let selected = json!({"context":"retained finite selection", "items":items});
        let original = prepared(selected.clone());
        let dirs = TempDir::new("finite-roster-binding");
        let mut gateway = McpGateway::new(
            vec![],
            PolicySet::from_yaml_str(
                "local.prepare_worklist: {class: read, resultArtifact: prepared}",
            )
            .unwrap(),
            dirs.path().join("cache"),
            dirs.path().join("artifacts"),
            None,
        )
        .unwrap()
        .with_argument_constraints(std::collections::BTreeMap::from([(
            "local.prepare_worklist".into(),
            json!({"type":"object","additionalProperties":false,
                    "required":["input"],"properties":{"input":{"const":original["input"]}}}),
        )]))
        .unwrap();
        let mut changed = selected.clone();
        changed["items"][0]["quantity"] = json!(9);
        let mut subset = selected.clone();
        subset["items"].as_array_mut().unwrap().pop();
        let mut reordered = selected.clone();
        reordered["items"].as_array_mut().unwrap().reverse();
        let mut rejected = vec![
            changed,
            subset,
            json!({"context":"retained finite selection","items":[]}),
        ];
        if count > 1 {
            rejected.push(reordered);
        }
        for value in rejected {
            let call = ToolCall {
                server: "local".into(),
                tool: "prepare_worklist".into(),
                args: prepared(value),
            };
            assert!(gateway.call(&call).is_err());
            assert!(!dirs.path().join("artifacts/prepared.json").exists());
        }
        let actual = gateway
            .call(&ToolCall {
                server: "local".into(),
                tool: "prepare_worklist".into(),
                args: original,
            })
            .unwrap();
        assert_eq!(
            actual.content_for_model["items"].as_array().unwrap().len(),
            count
        );
    }
}
