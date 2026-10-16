use trollm_core::plan::hex_sha256;
use trollm_core::ExecutionPlan;

const PLAN: &str = r#"
schemaVersion: ledger.execution-plan.v1
id: optional-provenance
version: 1
limits: { maxSteps: 1, maxReasonerCalls: 0, maxTokensTotal: 0, timeoutS: 60, artifactMaxMb: 1 }
onLimit: abort
models: { coder: { ctx: 2048 } }
entryStep: done
nodes:
  done: { kind: end, outcome: succeeded }
"#;

#[test]
fn omitted_provenance_loads_and_serializes_absent() {
    let plan = ExecutionPlan::from_yaml_str(PLAN).unwrap();
    assert!(plan.source_baseline.is_none());
    assert_eq!(plan.sha256, hex_sha256(PLAN.as_bytes()));
    assert!(serde_json::to_value(&plan)
        .unwrap()
        .get("sourceBaseline")
        .is_none());
    let yaml = serde_yaml::to_string(&plan).unwrap();
    assert!(!yaml.contains("sourceBaseline"));
    assert!(ExecutionPlan::from_yaml_str(&yaml)
        .unwrap()
        .source_baseline
        .is_none());
}

#[test]
fn present_provenance_survives_round_trip() {
    let hash = "a".repeat(64);
    let yaml = format!("{PLAN}sourceBaseline: {{ sha256: \"{hash}\" }}\n");
    let plan = ExecutionPlan::from_yaml_str(&yaml).unwrap();
    assert_eq!(plan.source_baseline.as_ref().unwrap().sha256, hash);
    assert_eq!(plan.sha256, hex_sha256(yaml.as_bytes()));
    let serialized = serde_yaml::to_string(&plan).unwrap();
    let reloaded = ExecutionPlan::from_yaml_str(&serialized).unwrap();
    assert_eq!(reloaded.source_baseline.unwrap().sha256, hash);
}

#[test]
fn malformed_present_provenance_is_rejected() {
    for value in [
        "null",
        "{}",
        "not-an-object",
        "[deadbeef]",
        "{ sha256: null }",
        "{ sha256: 123 }",
        "{ sha256: [] }",
    ] {
        let yaml = format!("{PLAN}sourceBaseline: {value}\n");
        assert!(
            ExecutionPlan::from_yaml_str(&yaml).is_err(),
            "accepted {value}"
        );
    }
}

#[test]
fn file_loader_keeps_the_raw_hash_without_provenance() {
    let dir = std::env::temp_dir().join(format!("trollm-provenance-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("optional-provenance.flow.yaml");
    std::fs::write(&path, PLAN).unwrap();
    let plan = ExecutionPlan::from_yaml_file(&path).unwrap();
    assert!(plan.source_baseline.is_none());
    assert_eq!(plan.sha256, hex_sha256(PLAN.as_bytes()));
    std::fs::remove_dir_all(dir).unwrap();
}
