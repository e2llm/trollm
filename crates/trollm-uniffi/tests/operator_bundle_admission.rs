//! Explicit operator admission check. Installs metadata only; never starts a run.
use std::path::PathBuf;
use trollm_core::{ExecutionPlan, TaskRegistry};
use trollm_uniffi::TrollmRuntime;

#[test]
#[ignore = "set TROLLM_OPERATOR_BUNDLE_PATH and run this test explicitly"]
fn operator_bundle_admits_without_models_or_mcp() {
    let path = PathBuf::from(
        std::env::var_os("TROLLM_OPERATOR_BUNDLE_PATH")
            .expect("TROLLM_OPERATOR_BUNDLE_PATH must name the bundle to validate"),
    );
    let bytes = std::fs::read(&path).expect("read the supplied operator bundle");
    let bundle_sha256 = trollm_core::plan::hex_sha256(&bytes);
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let base = std::env::temp_dir().join(format!(
        "trollm-operator-admission-{}-{nonce}",
        std::process::id()
    ));
    let runtime = TrollmRuntime::new(base.to_string_lossy().to_string(), None).unwrap();
    // Reuse the actual bridge installer: manifest, checksums, setup and core
    // admission are identical to phone import. No start_run/model/MCP calls.
    let installed = runtime.install_flow_bundle(bytes.clone()).unwrap();
    let setup = runtime.flow_setup(installed.flow_sha256.clone()).unwrap();
    if let Some(setup) = &setup {
        serde_json::from_str::<trollm_uniffi::setup::FlowSetup>(setup).unwrap();
    }
    let materialized = PathBuf::from(&installed.bundle_dir);
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(materialized.join("manifest.json")).unwrap())
            .unwrap();
    let plan =
        ExecutionPlan::from_yaml_file(&materialized.join(manifest["entryFlow"].as_str().unwrap()))
            .unwrap();
    let registry = TaskRegistry::from_yaml_file(
        &materialized.join(manifest["taskRegistry"].as_str().unwrap()),
    )
    .unwrap();
    trollm_core::validate::validate_plan(&plan).unwrap();
    trollm_core::validate::validate_with_registry(&plan, &registry).unwrap();
    assert_eq!(plan.id, installed.flow_id);
    assert_eq!(plan.version, installed.flow_version);
    assert_eq!(
        runtime.install_flow_bundle(bytes).unwrap().flow_sha256,
        installed.flow_sha256
    );
    println!(
        "{}",
        serde_json::json!({
            "bundleSha256": bundle_sha256, "flowId": installed.flow_id,
            "flowVersion": plan.version, "flowSha256": installed.flow_sha256,
            "registryId": registry.id, "registryVersion": registry.version,
            "filesVerified": installed.files_verified, "setupPresent": setup.is_some()
        })
    );
    drop(runtime);
    std::fs::remove_dir_all(base).unwrap();
}

#[test]
#[ignore = "set TROLLM_OPERATOR_BUNDLE_PATH; executes only a fixed observed native selection"]
fn operator_bundle_native_selection_publishes_complete_artifact() {
    use trollm_mcp::{McpGateway, PolicySet, ToolCall, ToolGateway};
    let path = PathBuf::from(
        std::env::var_os("TROLLM_OPERATOR_BUNDLE_PATH")
            .expect("TROLLM_OPERATOR_BUNDLE_PATH must name the bundle to validate"),
    );
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let base = std::env::temp_dir().join(format!(
        "trollm-operator-native-{}-{nonce}",
        std::process::id()
    ));
    let runtime = TrollmRuntime::new(base.to_string_lossy().to_string(), None).unwrap();
    let installed = runtime
        .install_flow_bundle(std::fs::read(path).unwrap())
        .unwrap();
    let materialized = PathBuf::from(&installed.bundle_dir);
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(materialized.join("manifest.json")).unwrap())
            .unwrap();
    let plan =
        ExecutionPlan::from_yaml_file(&materialized.join(manifest["entryFlow"].as_str().unwrap()))
            .unwrap();
    let registry = TaskRegistry::from_yaml_file(
        &materialized.join(manifest["taskRegistry"].as_str().unwrap()),
    )
    .unwrap();
    assert!(
        plan.mcp.servers.is_empty(),
        "operator native probe must not configure remote servers"
    );
    let task = registry
        .tasks
        .values()
        .find(|task| {
            task.tool_arguments
                .as_ref()
                .and_then(|rules| rules.get("local.select_unique"))
                .is_some_and(|rule| {
                    rule.fixed
                        .get("items")
                        .is_some_and(serde_json::Value::is_array)
                })
        })
        .expect("bundle must provide a fixed original collection for native selection");
    let rule = &task.tool_arguments.as_ref().unwrap()["local.select_unique"];
    let mut args = serde_json::Map::from_iter(rule.fixed.clone());
    let items = args["items"].clone();
    let field = args["field"].as_str().unwrap();
    let equals = format!(
        "{}{}",
        args["prefix"].as_str().unwrap(),
        items[0]
            .pointer(field)
            .and_then(serde_json::Value::as_str)
            .expect("first original record must expose the literal selection field")
    );
    args.insert("equals".into(), serde_json::Value::String(equals));
    let artifact_name = plan.mcp.policies["local.select_unique"]["resultArtifact"]
        .as_str()
        .expect("imported native policy must explicitly publish its success artifact");
    assert!(task
        .output_contract
        .require_artifact_complete
        .iter()
        .any(|name| name == artifact_name));
    let artifact_dir = base.join("native-artifacts");
    std::fs::create_dir_all(&artifact_dir).unwrap();
    let policies =
        PolicySet::from_yaml_value(&serde_yaml::to_value(&plan.mcp.policies).unwrap()).unwrap();
    let mut gateway =
        McpGateway::new(vec![], policies, base.join("cache"), artifact_dir, None).unwrap();
    let result = gateway
        .call(&ToolCall {
            server: "local".into(),
            tool: "select_unique".into(),
            args: serde_json::Value::Object(args),
        })
        .unwrap();
    assert_eq!(
        result.artifacts.len(),
        1,
        "native success must publish the current-call artifact"
    );
    let artifact = &result.artifacts[0];
    assert!(
        trollm_core::artifacts::is_complete(artifact),
        "native artifact sidecar must be complete"
    );
    let output: serde_json::Value =
        serde_json::from_slice(&std::fs::read(artifact).unwrap()).unwrap();
    assert_eq!(output, result.content_for_model);
    assert_eq!(output["selected"], items[0]);
    assert_eq!(
        output["sourceSha256"],
        trollm_mcp::CacheStore::hash_args(&items, None, None)
    );
    println!(
        "{}",
        serde_json::json!({"flowId": plan.id, "flowVersion": plan.version,
        "nativeArtifact": artifact_name, "complete": true, "sourceSha256": output["sourceSha256"]})
    );
    drop(gateway);
    drop(runtime);
    std::fs::remove_dir_all(base).unwrap();
}
