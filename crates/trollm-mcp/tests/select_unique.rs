#[allow(dead_code, unused_imports)]
mod common;

use common::TempDir;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use trollm_mcp::{local, EventSink, McpGateway, PolicySet, Provenance, ToolCall, ToolGateway};

fn args(items: Value) -> Value {
    json!({"items":items,"field":"/attributes/id","equals":"#scope","prefix":"#"})
}

fn hash(value: &Value) -> String {
    Sha256::digest(serde_json::to_vec(value).unwrap())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[test]
fn unique_selection_preserves_the_full_object_index_and_complete_source_hash() {
    let selected = json!({"id":"canonical-7","attributes":{"id":"scope"},"selector":"#scope","unknown":{"nested":[null,17,true]}});
    for items in [
        json!([{"attributes":{"id":"other"}}, selected]),
        json!([selected, {"attributes":{"id":"other"}}]),
    ] {
        let input = args(items.clone());
        let output = local::select_unique(&input).unwrap();
        assert_eq!(output["selected"], selected);
        assert_eq!(
            output["selected"],
            items[output["index"].as_u64().unwrap() as usize]
        );
        assert_eq!(output["sourceSha256"], hash(&items));
        assert_eq!(output.as_object().unwrap().len(), 3);
        assert_eq!(input["items"], items);
    }
}

#[test]
fn missing_nonstring_and_case_differences_do_not_match_and_duplicates_are_ambiguous() {
    for items in [
        json!([]),
        json!([{}]),
        json!([{"attributes":{"id":null}}, {"attributes":{"id":17}}]),
        json!([{"attributes":{"id":"Scope"}}]),
    ] {
        assert_eq!(
            local::select_unique(&args(items)).unwrap_err().to_string(),
            "local.select_unique: no-match"
        );
    }
    let item = json!({"id":"same","attributes":{"id":"scope"}});
    assert_eq!(
        local::select_unique(&args(json!([item, item])))
            .unwrap_err()
            .to_string(),
        "local.select_unique: ambiguous"
    );
}

#[test]
fn pointers_are_escaped_and_prefixes_are_literal_without_url_or_unicode_normalization() {
    let item = json!({"id":"canonical","a/b":{"~name":"a%2Fb"}});
    let mut input =
        json!({"items":[item],"field":"/a~1b/~0name","prefix":".*#","equals":".*#a%2Fb"});
    assert_eq!(
        local::select_unique(&input).unwrap()["selected"]["id"],
        "canonical"
    );
    input["equals"] = json!(".*#a/b");
    assert_eq!(
        local::select_unique(&input).unwrap_err().to_string(),
        "local.select_unique: no-match"
    );
    input["prefix"] = json!("");
    input["equals"] = json!("a%2Fb");
    assert!(local::select_unique(&input).is_ok());
    input["field"] = json!(""); // The root is an object, so it is a non-string nonmatch.
    assert_eq!(
        local::select_unique(&input).unwrap_err().to_string(),
        "local.select_unique: no-match"
    );
}

#[test]
fn malformed_arguments_and_invalid_json_pointers_fail_closed() {
    let good = args(json!([{"attributes":{"id":"scope"}}]));
    let mut invalid = vec![
        Value::Null,
        json!({}),
        json!({"items":[17],"field":"/x","prefix":"","equals":""}),
    ];
    for field in ["x", "#/x", "/x~", "/x~2", "/x~01~"] {
        let mut input = good.clone();
        input["field"] = json!(field);
        invalid.push(input);
    }
    for key in ["items", "field", "prefix", "equals"] {
        let mut input = good.clone();
        input.as_object_mut().unwrap().remove(key);
        invalid.push(input);
        let mut input = good.clone();
        input[key] = json!(false);
        invalid.push(input);
    }
    let mut input = good.clone();
    input["extra"] = json!(true);
    invalid.push(input);
    let mut input = good.clone();
    input["prefix"] = json!("é".repeat(8193));
    invalid.push(input);
    let mut input = good;
    input["field"] = json!(format!("/{}", "x".repeat(4096)));
    invalid.push(input);
    for input in invalid {
        assert_eq!(
            local::select_unique(&input).unwrap_err().to_string(),
            "local.select_unique: invalid-arguments"
        );
    }
}

#[test]
fn array_membership_keeps_original_object_identity_and_refuses_ambiguity() {
    let original = json!({"ids":["other","target","target"],"changes":[{"field":"value","to":"original query"}]});
    let input = json!({"items":[{"ids":["different"]},original],"field":"/ids","equals":"#target","prefix":"#","matchMode":"array-contains"});
    assert_eq!(local::select_unique(&input).unwrap()["selected"], original);
    for (items, expected) in [
        (json!([original, original]), "ambiguous"),
        (json!([{"ids":["target",17]}]), "no-match"),
        (json!([{"ids":"target"}]), "no-match"),
    ] {
        let mut value = input.clone();
        value["items"] = items;
        assert_eq!(
            local::select_unique(&value).unwrap_err().to_string(),
            format!("local.select_unique: {expected}")
        );
    }
    for mode in [json!(null), json!(true), json!("substring")] {
        let mut value = input.clone();
        value["matchMode"] = mode;
        assert!(local::select_unique(&value).is_err());
    }
    let mut scalar = args(json!([{"attributes":{"id":"scope"}}]));
    scalar["matchMode"] = json!("equals");
    assert!(local::select_unique(&scalar).is_ok());
}

#[test]
fn complete_arrays_are_never_truncated_and_oversized_sources_are_rejected() {
    let large = args(
        json!([{"attributes":{"id":"other"},"body":"x".repeat(300_000)}, {"id":"last","attributes":{"id":"scope"}}]),
    );
    assert_eq!(local::select_unique(&large).unwrap()["index"], 1);
    let oversized = args(json!([{"attributes":{"id":"scope"},"body":"x".repeat(32*1024*1024)}]));
    assert!(local::select_unique(&oversized)
        .unwrap_err()
        .to_string()
        .contains("serialized size limit"));
    let too_many = args(json!(vec![json!({}); 100_001]));
    assert_eq!(
        local::select_unique(&too_many).unwrap_err().to_string(),
        "local.select_unique: invalid-arguments"
    );
    let mut nested = json!("scope");
    for _ in 0..65 {
        nested = json!({"nested":nested});
    }
    let deep = args(json!([nested]));
    assert!(local::select_unique(&deep)
        .unwrap_err()
        .to_string()
        .contains("nesting limit"));
}

#[test]
fn gateway_catalog_schema_constraints_artifacts_and_response_bytes_use_existing_paths() {
    let dirs = TempDir::new("select-unique-gateway");
    let captured = Arc::new(Mutex::new(Vec::<Value>::new()));
    let sink_events = captured.clone();
    let sink: EventSink = Box::new(move |event| {
        sink_events
            .lock()
            .unwrap()
            .push(serde_json::to_value(event)?);
        Ok(())
    });
    let policies = PolicySet::from_yaml_str(&json!({"local.select_unique":{
        "resultArtifact":"selection", "resultValidation":{"maxBytes":262144,
        "schema":serde_json::from_str::<Value>(include_str!("../../../schemas/local-select-unique.v1.output.schema.json")).unwrap()}
    }}).to_string()).unwrap();
    let mut gateway = McpGateway::new(
        vec![],
        policies,
        dirs.path().join("cache"),
        dirs.path().join("artifacts"),
        Some(sink),
    )
    .unwrap()
    .with_argument_constraints(BTreeMap::from([(
        "local.select_unique".into(),
        json!({"properties":{"field":{"const":"/attributes/id"}}}),
    )]))
    .unwrap();
    assert!(gateway
        .list_tools()
        .unwrap()
        .iter()
        .any(|tool| tool.server == "local" && tool.name == "select_unique"));
    let call = ToolCall {
        server: "local".into(),
        tool: "select_unique".into(),
        args: args(json!([
            {"attributes":{"id":"other"},"body":"x".repeat(300_000)},
            {"id":"kept","attributes":{"id":"scope"},"original":[1,null]}
        ])),
    };
    let result = gateway.call(&call).unwrap();
    assert_eq!(result.provenance, Provenance::Local);
    assert_eq!(
        result.content_for_model,
        local::select_unique(&call.args).unwrap()
    );
    let saved: Value = serde_json::from_slice(
        &std::fs::read(dirs.path().join("artifacts/selection.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(saved, result.content_for_model);
    let events = captured.lock().unwrap();
    for stage in ["raw_payload", "gateway_result"] {
        assert!(events.iter().any(|event| event["type"] == "response_size"
            && event["stage"] == stage
            && event["compact_json_utf8_bytes"]
                == serde_json::to_vec(&saved).unwrap().len() as u64));
    }
    drop(events);
    let mut invalid = call.clone();
    invalid.args["field"] = json!("/other");
    assert!(gateway.validate_call(&invalid).is_err());
    invalid.args = call.args.clone();
    invalid.args["unknown"] = json!(true);
    assert!(gateway.call(&invalid).is_err());
    let ordinary = ToolCall {
        server: "local".into(),
        tool: "calculate".into(),
        args: json!({"operation":"add","left":"x".repeat(300_000),"right":"1"}),
    };
    assert!(gateway
        .validate_call(&ordinary)
        .unwrap_err()
        .to_string()
        .contains("serialized size limit"));
}

#[test]
fn failed_selection_publishes_no_selected_artifact_and_cannot_use_mutation_or_cache_policy() {
    for items in [
        json!([]),
        json!([{"attributes":{"id":"scope"}},{"attributes":{"id":"scope"}}]),
    ] {
        let dirs = TempDir::new("select-unique-failure");
        let policies =
            PolicySet::from_yaml_str("local.select_unique: {resultArtifact: selection}").unwrap();
        let mut gateway = McpGateway::new(
            vec![],
            policies,
            dirs.path().join("cache"),
            dirs.path().join("artifacts"),
            None,
        )
        .unwrap();
        assert!(gateway
            .call(&ToolCall {
                server: "local".into(),
                tool: "select_unique".into(),
                args: args(items)
            })
            .is_err());
        assert!(!dirs.path().join("artifacts/selection.json").exists());
    }
    for policy in [
        json!({"class":"mutation"}),
        json!({"cache":{"ttlS":30}}),
        json!({"passthrough":true}),
    ] {
        let dirs = TempDir::new("select-unique-policy");
        let policies =
            PolicySet::from_yaml_str(&json!({"local.select_unique":policy}).to_string()).unwrap();
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
