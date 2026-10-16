use trollm_core::plan::hex_sha256;
use trollm_core::ExecutionPlan;

const PLAN: &str = r#"
schemaVersion: ledger.execution-plan.v1
id: setup-test
version: 1
limits: { maxSteps: 1, maxReasonerCalls: 0, maxTokensTotal: 100, timeoutS: 60, artifactMaxMb: 1 }
onLimit: abort
models: { coder: { ctx: 2048 } }
entryStep: done
nodes: { done: { kind: end, outcome: succeeded } }
"#;

#[test]
fn setup_pin_is_optional_and_changes_raw_flow_identity() {
    let legacy = ExecutionPlan::from_yaml_str(PLAN).unwrap();
    assert!(legacy.setup.is_none());
    assert!(serde_json::to_value(&legacy)
        .unwrap()
        .get("setup")
        .is_none());
    let first = format!(
        "{PLAN}setup: {{ path: setup.json, sha256: '{}' }}\n",
        "a".repeat(64)
    );
    let second = first.replace(&"a".repeat(64), &"b".repeat(64));
    let plan = ExecutionPlan::from_yaml_str(&first).unwrap();
    assert_eq!(plan.sha256, hex_sha256(first.as_bytes()));
    assert_ne!(
        plan.sha256,
        ExecutionPlan::from_yaml_str(&second).unwrap().sha256
    );
    assert_eq!(plan.setup.unwrap().path, "setup.json");
}

#[test]
fn invalid_setup_pins_and_types_reject_at_parse_time() {
    for value in ["null", "[]", "{}", "{ path: setup.json, sha256: 123 }"] {
        assert!(ExecutionPlan::from_yaml_str(&format!("{PLAN}setup: {value}\n")).is_err());
    }
    for path in [
        "",
        "/setup.json",
        "../setup.json",
        "a/../setup.json",
        "./setup.json",
        "a//b",
        "C:setup.json",
        "a\\b",
    ] {
        let reference = serde_json::json!({"path":path,"sha256":"a".repeat(64)});
        let yaml = format!("{PLAN}setup: {reference}\n");
        assert!(
            ExecutionPlan::from_yaml_str(&yaml).is_err(),
            "accepted path {path:?}"
        );
    }
    for hash in ["A".repeat(64), "a".repeat(63), "g".repeat(64)] {
        let reference = serde_json::json!({"path":"setup.json","sha256":hash});
        assert!(ExecutionPlan::from_yaml_str(&format!("{PLAN}setup: {reference}\n")).is_err());
    }
}

#[test]
fn setup_is_root_only() {
    let mut root = ExecutionPlan::from_yaml_str(PLAN).unwrap();
    let mut child = ExecutionPlan::from_yaml_str(&format!(
        "{PLAN}setup: {{ path: setup.json, sha256: '{}' }}\n",
        "a".repeat(64)
    ))
    .unwrap();
    child.id = "child".into();
    root.subflows.insert(child.id.clone(), child);
    let error = trollm_core::validate::validate_plan(&root)
        .unwrap_err()
        .to_string();
    assert!(error.contains("root flow"), "{error}");
}
