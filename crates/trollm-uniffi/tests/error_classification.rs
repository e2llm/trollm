use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use trollm_uniffi::{TrollmError, TrollmRuntime};

struct TestDir(PathBuf);

impl TestDir {
    fn new(label: &str) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "trollm-error-classification-{label}-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn runtime(&self) -> std::sync::Arc<TrollmRuntime> {
        TrollmRuntime::new(self.0.to_string_lossy().into_owned(), None).unwrap()
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}

fn tar(files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut bytes = Vec::new();
    for (name, data) in files {
        let mut header = [0u8; 512];
        header[..name.len()].copy_from_slice(name.as_bytes());
        header[100..108].copy_from_slice(b"0000644\0");
        header[108..116].copy_from_slice(b"0000000\0");
        header[116..124].copy_from_slice(b"0000000\0");
        header[124..136].copy_from_slice(format!("{:011o}\0", data.len()).as_bytes());
        header[136..148].copy_from_slice(b"00000000000\0");
        header[156] = b'0';
        header[257..262].copy_from_slice(b"ustar");
        header[148..156].fill(b' ');
        let sum: u64 = header.iter().map(|byte| u64::from(*byte)).sum();
        header[148..156].copy_from_slice(format!("{:06o}\0 ", sum).as_bytes());
        bytes.extend_from_slice(&header);
        bytes.extend_from_slice(data);
        bytes.resize(bytes.len().div_ceil(512) * 512, 0);
    }
    bytes.resize(bytes.len() + 1024, 0);
    bytes
}

fn manifest_for_weights() -> Vec<u8> {
    format!(
        "{{\"format\":\"trollm.flow-bundle.v1\",\"entryFlow\":\"flow.flow.yaml\",\"taskRegistry\":\"task.tasks.yaml\",\"files\":[{{\"path\":\"weights.txt\",\"sha256\":\"{}\"}}]}}",
        "0".repeat(64)
    )
    .into_bytes()
}

#[test]
fn missing_weights_named_bundle_member_is_a_bundle_error() {
    let base = TestDir::new("missing-weights");
    let error = base
        .runtime()
        .install_flow_bundle(tar(&[("manifest.json", &manifest_for_weights())]))
        .unwrap_err();
    assert!(matches!(&error, TrollmError::Bundle(_)), "{error}");
    assert!(error.to_string().contains("weights.txt"));
}

#[test]
fn checksum_failed_weights_named_bundle_member_is_a_bundle_error() {
    let base = TestDir::new("checksum-weights");
    let manifest = manifest_for_weights();
    let error = base
        .runtime()
        .install_flow_bundle(tar(&[("manifest.json", &manifest), ("weights.txt", b"x")]))
        .unwrap_err();
    assert!(matches!(&error, TrollmError::Bundle(_)), "{error}");
    assert!(error
        .to_string()
        .contains("sha256 mismatch for 'weights.txt'"));
}

#[test]
fn missing_installed_flow_setup_is_not_found() {
    let base = TestDir::new("missing-flow-weights-manifest");
    let error = base.runtime().flow_setup("0".repeat(64)).unwrap_err();
    assert!(matches!(&error, TrollmError::NotFound(_)), "{error}");
    assert!(error.to_string().contains("reading installed manifest"));
}

#[cfg(not(any(feature = "stub", feature = "llama")))]
#[test]
fn unavailable_runtime_keeps_its_category_in_a_bundle_named_path() {
    let base = TestDir::new("bundle-manifest-weights-sha256");
    let error = base
        .runtime()
        .start_run("0".repeat(64), "hello".into())
        .unwrap_err();
    assert!(
        matches!(&error, TrollmError::ModelUnavailable(_)),
        "{error}"
    );
    assert!(error.to_string().contains("no model runtime"));
}

#[cfg(any(feature = "stub", feature = "llama"))]
#[test]
fn missing_installed_flow_at_run_start_is_not_found() {
    let base = TestDir::new("bundle-manifest-weights-sha256");
    let error = base
        .runtime()
        .start_run("0".repeat(64), "hello".into())
        .unwrap_err();
    assert!(matches!(&error, TrollmError::NotFound(_)), "{error}");
    assert!(error.to_string().contains("no installed flow bundle"));
}
