//! Literal processing and the production gateway, with no network or model.
#[allow(dead_code, unused_imports)]
mod common;

use common::TempDir;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use trollm_mcp::{local, McpGateway, PolicySet, Provenance, ToolCall, ToolGateway};

fn args(primary: &str, secondary: &str) -> Value {
    json!({"primary": primary, "secondary": secondary,
        "rules": {"prefix": "$", "fractionDigits": 2}})
}

fn error(args: &Value, detail: &str) {
    assert_eq!(
        local::extract_decimal(args).unwrap_err().to_string(),
        format!("local.extract_decimal: {detail}")
    );
}

#[test]
fn exact_matches_preserve_original_source_hashes_and_report_one_source_honestly() {
    for (primary, secondary) in [("$2.90", "$2 90"), ("$2.90", ""), ("", "$2.90")] {
        let output = local::extract_decimal(&args(primary, secondary)).unwrap();
        assert_eq!(output["decimal"], "2.9");
        for (index, text) in [primary, secondary].iter().enumerate() {
            use sha2::{Digest, Sha256};
            assert_eq!(
                output["sources"][index]["textSha256"],
                format!("{:x}", Sha256::digest(text.as_bytes()))
            );
            assert_eq!(
                output["sources"][index]["status"],
                if text.is_empty() {
                    "unmatched"
                } else {
                    "matched"
                }
            );
        }
        let schema: Value = serde_json::from_str(include_str!(
            "../../../schemas/local-extract-decimal.v1.output.schema.json"
        ))
        .unwrap();
        assert!(jsonschema::options()
            .offline()
            .build(&schema)
            .unwrap()
            .is_valid(&output));
    }
    let mut maximum = args("$999999999999.999999", "");
    maximum["rules"]["fractionDigits"] = json!(6);
    assert_eq!(
        local::extract_decimal(&maximum).unwrap()["decimal"],
        "999999999999.999999"
    );
}

#[test]
fn ambiguous_equal_repeats_conflicts_and_missing_sources_fail_closed() {
    error(&args("$2.90 $2.90", ""), "ambiguous-source");
    error(&args("", "$2.90 $2.90"), "ambiguous-source");
    error(&args("$2.90", "$2.91"), "sources-disagree");
    for text in [
        "",
        "none",
        "$02.90",
        "$2.9",
        "$2.900",
        "$2.90.1",
        "$1000000000000.00",
        "$2,90",
    ] {
        error(&args(text, ""), "no-match");
    }
    let mut missing = args("$2.90", "");
    missing.as_object_mut().unwrap().remove("secondary");
    error(&missing, "invalid-arguments");
}

#[test]
fn authored_windows_stops_and_bounds_are_literal_and_bounded() {
    let mut request = args(
        "$99.00 START $2.90 Shipping $9.00 END $88.00",
        "START missing END",
    );
    request["rules"] = json!({"prefix":"$", "fractionDigits":2, "after":"START", "before":"END", "stopBefore":["shipping"], "minimumExclusive":"2.89"});
    assert_eq!(local::extract_decimal(&request).unwrap()["decimal"], "2.9");
    request["rules"]["minimumExclusive"] = json!("2.90");
    error(&request, "outside-authored-bound");
    // A source that never matches has no numeric value to check against the bound.
    request["primary"] = json!("START missing END");
    request["secondary"] = json!("START $3.00 END");
    assert_eq!(
        local::extract_decimal(&request).unwrap()["sources"][0]["status"],
        "unmatched"
    );
    request["secondary"] = json!("$3.00");
    error(&request, "no-match");
}

#[test]
fn malformed_and_oversized_inputs_are_rejected_standalone_and_by_gateway_admission() {
    let dirs = TempDir::new("extract-admission");
    let mut gateway = McpGateway::new(
        vec![],
        PolicySet::default(),
        dirs.path().join("cache"),
        dirs.path().join("artifacts"),
        None,
    )
    .unwrap();
    let mut invalid = vec![json!(null), json!({}), args("$2.90", "")];
    invalid.last_mut().unwrap()["extra"] = json!(true);
    for (key, value) in [
        ("prefix", json!("")),
        ("prefix", json!("é".repeat(129))),
        ("fractionDigits", json!(0)),
        ("fractionDigits", json!(7)),
        ("fractionDigits", json!(1.5)),
        ("minimumExclusive", json!("01")),
        ("minimumExclusive", json!("1\n")),
        ("stopBefore", json!([""])),
        ("stopBefore", json!(vec!["x"; 17])),
        ("unknown", json!(true)),
    ] {
        let mut request = args("$2.90", "");
        request["rules"][key] = value;
        invalid.push(request);
    }
    for text in ["x".repeat(16385), "é".repeat(8193)] {
        invalid.push(args(&text, ""));
        invalid.push(args("", &text));
    }
    for request in invalid {
        error(&request, "invalid-arguments");
        let call = ToolCall {
            server: "local".into(),
            tool: "extract_decimal".into(),
            args: request,
        };
        assert!(gateway.validate_call(&call).is_err());
        assert!(gateway.call(&call).is_err());
    }
    assert!(!dirs.path().join("artifacts").exists());
    let source = format!("{}$2.90", "x".repeat(16379));
    assert_eq!(
        local::extract_decimal(&args(&source, "")).unwrap()["decimal"],
        "2.9"
    );
}

#[test]
fn production_gateway_applies_constraints_and_preserves_unshaped_extraction_artifact() {
    let dirs = TempDir::new("extract-gateway");
    let policies = PolicySet::from_yaml_str(&json!({"local.extract_decimal": {
        "resultArtifact":"extraction", "shape":{"strip":["sources"]},
        "resultValidation":{"maxBytes":2048,"schema":serde_json::from_str::<Value>(include_str!("../../../schemas/local-extract-decimal.v1.output.schema.json")).unwrap()}
    }}).to_string()).unwrap();
    let mut gateway = McpGateway::new(
        vec![],
        policies,
        dirs.path().join("cache"),
        dirs.path().join("artifacts"),
        None,
    )
    .unwrap()
    .with_argument_constraints(BTreeMap::from([(
        "local.extract_decimal".into(),
        json!({"properties":{"rules":{"const":{"prefix":"$","fractionDigits":2}}}}),
    )]))
    .unwrap();
    assert!(gateway
        .list_tools()
        .unwrap()
        .iter()
        .any(|tool| tool.server == "local" && tool.name == "extract_decimal"));
    let mut call = ToolCall {
        server: "local".into(),
        tool: "extract_decimal".into(),
        args: args("$2.90", ""),
    };
    let result = gateway.call(&call).unwrap();
    assert_eq!(result.provenance, Provenance::Local);
    assert!(
        result.content_for_model["sources"]["bytes"]
            .as_u64()
            .unwrap()
            > 0
    );
    assert!(result.content_for_model["sources"].is_object());
    let original: Value = serde_json::from_slice(
        &std::fs::read(dirs.path().join("artifacts/extraction.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(original, local::extract_decimal(&call.args).unwrap());
    call.args["rules"]["prefix"] = json!("USD");
    assert!(gateway.validate_call(&call).is_err());
    assert!(gateway.call(&call).is_err());
}

#[test]
fn extraction_policy_cannot_enable_mutation_transport_or_cache_paths() {
    let dirs = TempDir::new("extract-policy");
    for policy in [
        json!({"class":"mutation"}),
        json!({"cache":{"ttlS":30}}),
        json!({"passthrough":true}),
        json!({"serveFrom":{"fetchedBy":"remote.list","match":"id"}}),
        json!({"onMiss":"fail"}),
        json!({"paginate":{"param":"cursor","deliver":"first_page","backgroundFetch":"all"}}),
    ] {
        let policies =
            PolicySet::from_yaml_str(&json!({"local.extract_decimal":policy}).to_string()).unwrap();
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
