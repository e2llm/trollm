//! The published historical example must remain importable without inference.
use trollm_uniffi::TrollmRuntime;

#[test]
fn published_v342_bundle_installs_without_models_or_connections() {
    let bytes = include_bytes!("../../../examples/run-342/amazon-cart-v342.tar");
    assert_eq!(
        trollm_core::plan::hex_sha256(bytes),
        "5ab7385fa61e88470ad408458faf15096d116c202d57ea783f7652945b2e7e83"
    );
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let directory = std::env::temp_dir().join(format!(
        "trollm-published-v342-{}-{nonce}",
        std::process::id()
    ));
    let runtime = TrollmRuntime::new(directory.to_string_lossy().to_string(), None).unwrap();
    let installed = runtime.install_flow_bundle(bytes.to_vec()).unwrap();
    assert_eq!(installed.flow_id, "amazon-cart");
    assert_eq!(installed.flow_version, 342);
    assert!(runtime
        .flow_setup(installed.flow_sha256.clone())
        .unwrap()
        .is_some());
    assert_eq!(
        runtime
            .install_flow_bundle(bytes.to_vec())
            .unwrap()
            .flow_sha256,
        installed.flow_sha256
    );
    drop(runtime);
    std::fs::remove_dir_all(directory).unwrap();
}
