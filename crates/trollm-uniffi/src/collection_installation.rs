//! App-private selected installer input. Never accepted from a source bundle,
//! model arguments, exported Android intent or public publication descriptor.
use anyhow::{ensure, Context, Result};
use serde::Deserialize;
use serde_json::Value;
use std::io::Read;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use trollm_mcp::private_guard::PrivateGuardClient;
use trollm_mcp::transport::RootCertificates;

const MAX_INSTALLATION_BYTES: u64 = 262_144;
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PrivateInstallation {
    schema: String,
    installation_sha256: String,
    ready: Value,
    guard_url: String,
    guard_token: String,
    guard_roots_pem: String,
    selected_device_max_error_us: u64,
    expected_static_key_sha256: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PrivateCalibratedInstallation {
    schema: String,
    installation_sha256: String,
    ready: Value,
    guard_url: String,
    guard_token: String,
    guard_roots_pem: String,
    clock_policy: crate::collection_clock::CalibratedClockPolicy,
    expected_static_key_sha256: String,
}

fn checked_read(base: &Path, key: &str) -> Result<Vec<u8>> {
    ensure!(
        key.len() == 64
            && key
                .bytes()
                .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c)),
        "private selected installation key must be lowercase SHA256"
    );
    let parent = base.join("selected-installations");
    let base_meta = std::fs::symlink_metadata(base)?;
    let parent_meta = std::fs::symlink_metadata(&parent)?;
    ensure!(
        base_meta.is_dir()
            && parent_meta.is_dir()
            && !parent_meta.file_type().is_symlink()
            && parent_meta.uid() == base_meta.uid()
            && parent_meta.permissions().mode() & 0o077 == 0,
        "selected installation directory is not app-private"
    );
    ensure!(
        parent.canonicalize()? == base.canonicalize()?.join("selected-installations"),
        "selected installation parent changed"
    );
    let path = parent.join(format!("{key}.json"));
    let before = std::fs::symlink_metadata(&path)?;
    ensure!(
        before.is_file()
            && !before.file_type().is_symlink()
            && before.uid() == base_meta.uid()
            && before.permissions().mode() & 0o077 == 0
            && before.len() <= MAX_INSTALLATION_BYTES,
        "selected installation file is not bounded app-private input"
    );
    let mut file = std::fs::File::open(&path)?;
    let opened = file.metadata()?;
    ensure!(
        opened.dev() == before.dev() && opened.ino() == before.ino(),
        "selected installation replaced during open"
    );
    let mut bytes = Vec::new();
    file.by_ref()
        .take(MAX_INSTALLATION_BYTES + 1)
        .read_to_end(&mut bytes)?;
    let after = file.metadata()?;
    let named = std::fs::symlink_metadata(&path)?;
    ensure!(
        bytes.len() as u64 == before.len()
            && bytes.len() as u64 <= MAX_INSTALLATION_BYTES
            && opened.len() == after.len()
            && opened.mtime() == after.mtime()
            && opened.mtime_nsec() == after.mtime_nsec()
            && named.dev() == opened.dev()
            && named.ino() == opened.ino(),
        "selected installation changed during read"
    );
    ensure!(
        crate::collection::raw_sha256(&bytes) == key,
        "selected installation original bytes differ from private key"
    );
    Ok(bytes)
}

/// The operator/launcher must provision this exact app-private file and exclusive
/// guard credential. Its SHA names bytes, not authenticated native provenance.
/// The ordinary source-run entry never reads this directory or opts in itself.
pub fn load(
    base: &Path,
    key: &str,
    installation_sha256: &str,
    invocation_key: &str,
) -> Result<crate::collection_provider::SelectedRunInstallation> {
    let bytes = checked_read(base, key)?;
    let schema: Value =
        serde_json::from_slice(&bytes).context("invalid private selected installation")?;
    let (installation, ready, url, token, roots, expected_key, kernel, calibrated) =
        match schema["schema"].as_str() {
            Some("trollm.private-selected-installation.v1") => {
                let input: PrivateInstallation = serde_json::from_slice(&bytes)?;
                ensure!(
                    input.schema == "trollm.private-selected-installation.v1",
                    "private installation schema mismatch"
                );
                (
                    input.installation_sha256,
                    input.ready,
                    input.guard_url,
                    input.guard_token,
                    input.guard_roots_pem,
                    input.expected_static_key_sha256,
                    Some(input.selected_device_max_error_us),
                    None,
                )
            }
            Some("trollm.private-selected-installation.v2-proposal") => {
                let input: PrivateCalibratedInstallation = serde_json::from_slice(&bytes)?;
                ensure!(
                    input.schema == "trollm.private-selected-installation.v2-proposal",
                    "private installation schema mismatch"
                );
                (
                    input.installation_sha256,
                    input.ready,
                    input.guard_url,
                    input.guard_token,
                    input.guard_roots_pem,
                    input.expected_static_key_sha256,
                    None,
                    Some(input.clock_policy),
                )
            }
            _ => anyhow::bail!("private selected installation schema unsupported"),
        };
    ensure!(
        installation == installation_sha256
            && !invocation_key.is_empty()
            && ready["invocationKey"].as_str() == Some(invocation_key),
        "private selected installation source mismatch"
    );
    ensure!(
        (16..=4096).contains(&token.len()) && !token.bytes().any(|c| c <= 0x20 || c >= 0x7f),
        "private guard credential invalid"
    );
    let roots = RootCertificates::from_pem(roots.as_bytes())?;
    let getter = pinned_token(base.to_owned(), key.to_owned(), token);
    let client = PrivateGuardClient::new(&url, getter, roots)?;
    let installed = if let Some(policy) = calibrated {
        crate::collection_provider::SelectedRunInstallation::new_calibrated(
            ready,
            client,
            policy,
            expected_key,
        )?
    } else {
        crate::collection_provider::SelectedRunInstallation::new(
            ready,
            client,
            kernel.context("kernel clock policy unavailable")?,
            expected_key,
        )?
    };
    Ok(installed.with_private_installation_key(key.to_owned()))
}

fn pinned_token(path: PathBuf, key: String, token: String) -> trollm_mcp::transport::TokenGetter {
    let failed = AtomicBool::new(false);
    Arc::new(move || {
        if failed.load(Ordering::SeqCst) {
            return None;
        }
        if checked_read(&path, &key).is_err() {
            failed.store(true, Ordering::SeqCst);
            return None;
        }
        Some(token.clone())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[cfg(feature = "stub")]
    fn supported_selected_entry_retains_admitted_originals_before_static_auth_refusal() {
        use crate::collection::raw_sha256;
        use crate::collection_members::{
            CollectionMembers, CollectionSource, MATCHED_PROFILE, SOURCE_SCHEMA,
        };
        use serde_json::json;
        struct Unavailable;
        impl crate::TokenProvider for Unavailable {
            fn bearer_token(&self, _: String, _: String) -> Option<String> {
                None
            }
            fn connection_auth(&self, _: String, _: String) -> crate::ConnectionAuth {
                crate::ConnectionAuth::Unavailable
            }
        }
        #[allow(dead_code)]
        mod tls {
            include!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../trollm-mcp/tests/common/tls_fixture.rs"
            ));
        }
        let root = std::env::temp_dir().join(format!(
            "selected-before-bind-{}",
            trollm_core::RunDir::under(Path::new("unused")).run_id()
        ));
        let runtime =
            crate::TrollmRuntime::new(root.to_string_lossy().into(), Some(Box::new(Unavailable)))
                .unwrap();
        let base = root.join("trollm");
        let mut body = crate::collection::tests::body();
        body["contextRef"]["sha256"] = json!(raw_sha256(b"declared test context"));
        let selection = crate::collection::tests::decode(&body).unwrap();
        let members = CollectionMembers::compile_matched(
            &selection,
            b"declared test context",
            "https://api.e2llm.dev/mcp",
        )
        .unwrap();
        let installation = raw_sha256(&members.files["manifest.json"]);
        let flow = raw_sha256(&members.files["selected-collection.flow.yaml"]);
        std::fs::create_dir_all(base.join("bundle-revisions")).unwrap();
        members
            .write_new(&base.join("bundle-revisions").join(&installation))
            .unwrap();
        let source = CollectionSource {
            schema_version: SOURCE_SCHEMA.into(),
            origin: "DECLARED".into(),
            owner: selection.selection.owner.clone(),
            fixture_id: "declared-fixture".into(),
            version_id: "declared-version".into(),
            selection_ref: selection.selection_ref.clone(),
            selection_raw_sha256: raw_sha256(&selection.original_bytes),
            profile_id: MATCHED_PROFILE.into(),
            profile_sha256: raw_sha256(&members.files["collection-profile.json"]),
            installation_sha256: installation.clone(),
            qualification: "unqualified".into(),
        };
        let input = serde_json::to_string(
            &json!({"selectionSha256":raw_sha256(&selection.original_bytes)}),
        )
        .unwrap();
        let admission = runtime
            .admit_source_input(installation.clone(), input.clone(), input.clone())
            .unwrap();
        let association = json!({"inputAdmission":serde_json::from_str::<Value>(&admission).unwrap(),
            "installation":{"installationSha256":installation,"flowSha256":flow,"archiveSha256":"declared-container"},
            "descriptor":{"source":source,"identity":{"manifestSha256":installation,"entryFlowSha256":flow,"sha256":"declared-container"}}});
        let configuration = json!({"version":1,"models":{"coder":{"path":"/declared/unopened.gguf",
            "sha256":"bd258782e35f7f458f8aced1adc053e6e92e89bc735ba3be89d38a06121dc517","ctx":4096,"temperature":0.2,"topP":0.95}},
            "connections":{"e2llm":{"url":"https://api.e2llm.dev/mcp"}}});
        let private = json!({"schema":"trollm.private-selected-installation.v1","installationSha256":installation,
            "ready":{"invocationKey":selection.selection.invocation_key},"guardUrl":"https://localhost:9/guard",
            "guardToken":"declared-unused-guard","guardRootsPem":std::str::from_utf8(tls::ROOT_1_PEM).unwrap(),"selectedDeviceMaxErrorUs":1,
            "expectedStaticKeySha256":"a".repeat(64)});
        let bytes = serde_json::to_vec(&private).unwrap();
        let key = raw_sha256(&bytes);
        let parent = base.join("selected-installations");
        std::fs::create_dir(&parent).unwrap();
        std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o700)).unwrap();
        let file = parent.join(format!("{key}.json"));
        std::fs::write(&file, bytes).unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
        let error = runtime
            .start_selected_source_run_configured(
                flow.clone(),
                installation.clone(),
                input.clone(),
                configuration.to_string(),
                association.to_string(),
                key.clone(),
                selection.selection.invocation_key.clone(),
            )
            .unwrap_err();
        let id = match error {
            crate::TrollmError::SelectedStart {
                code,
                native_run_id: Some(id),
            } => {
                assert_eq!(code, "SELECTED_BIND_STATIC_KEY_MODE");
                id
            }
            other => panic!("typed allocated start failure required: {other:?}"),
        };
        let directory = base.join("runs").join(&id);
        assert_eq!(
            std::fs::read(directory.join("input.original.json")).unwrap(),
            input.as_bytes()
        );
        assert_eq!(
            std::fs::read(directory.join("input.normalized.json")).unwrap(),
            input.as_bytes()
        );
        assert_eq!(
            serde_json::from_slice::<Value>(
                &std::fs::read(directory.join("input-admission.json")).unwrap()
            )
            .unwrap(),
            serde_json::from_str::<Value>(&admission).unwrap()
        );
        let retained: Value = serde_json::from_slice(
            &std::fs::read(directory.join("source-association.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(retained["nativeRunId"], id);
        assert_eq!(retained["inputAdmission"], association["inputAdmission"]);
        assert!(directory.join("configuration.json").is_file());
        assert!(!directory.join("events.jsonl").exists());
        assert!(!directory.join("state.snapshot.json").exists());
        assert!(runtime.selected_run_binding(id).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn actual_private_v2_loader_keeps_kernel_policy_disjoint_and_refuses_mixed_fields() {
        let base = std::env::temp_dir().join(format!(
            "private-clock-v2-{}",
            trollm_core::RunDir::under(Path::new("unused")).run_id()
        ));
        std::fs::create_dir(&base).unwrap();
        std::fs::set_permissions(&base, std::fs::Permissions::from_mode(0o700)).unwrap();
        let parent = base.join("selected-installations");
        std::fs::create_dir(&parent).unwrap();
        std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o700)).unwrap();
        let original = serde_json::json!({"schema":"trollm.private-selected-installation.v2-proposal",
            "installationSha256":"a".repeat(64),"ready":{"invocationKey":"original"},
            "guardUrl":"https://localhost:9/guard","guardToken":"declared-v2-token-only",
            "guardRootsPem":"assigned-from-existing-test-fixture",
            "expectedStaticKeySha256":"b".repeat(64),"clockPolicy":{
                "profileId":"trollm.android-selected-emulator-calibrated-clock.v1-proposal",
                "guestBootId":"11111111-2222-4333-8444-555555555555",
                "minimumRateNumerator":1,"minimumRateDenominator":2,"maxCalibrationAgeMs":10000}});
        let mut original = original;
        #[allow(dead_code)]
        mod tls {
            include!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../trollm-mcp/tests/common/tls_fixture.rs"
            ));
        }
        original["guardRootsPem"] =
            serde_json::json!(std::str::from_utf8(tls::ROOT_1_PEM).unwrap());
        let install = |value: &Value| {
            let bytes = serde_json::to_vec(value).unwrap();
            let key = crate::collection::raw_sha256(&bytes);
            let file = parent.join(format!("{key}.json"));
            std::fs::write(&file, bytes).unwrap();
            std::fs::set_permissions(file, std::fs::Permissions::from_mode(0o600)).unwrap();
            load(&base, &key, &"a".repeat(64), "original")
        };
        // Loader/credential construction only, no network, run, or guest support.
        assert!(install(&original).is_ok());
        let mut mixed = original.clone();
        mixed["selectedDeviceMaxErrorUs"] = serde_json::json!(1);
        assert!(install(&mixed).is_err());
        mixed = original.clone();
        mixed["schema"] = serde_json::json!("trollm.private-selected-installation.v1");
        assert!(install(&mixed).is_err());
        mixed = original;
        mixed["clockPolicy"]["guestBootId"] = serde_json::json!("uploaded-boot");
        assert!(install(&mixed).is_err());
        std::fs::remove_dir_all(base).unwrap();
    }
    #[test]
    fn private_file_identity_permissions_and_replacement_refuse() {
        let base = std::env::temp_dir().join(format!(
            "selected-installation-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let parent = base.join("selected-installations");
        std::fs::create_dir_all(&parent).unwrap();
        std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o700)).unwrap();
        let bytes = b"private declared control";
        let key = crate::collection::raw_sha256(bytes);
        let path = parent.join(format!("{key}.json"));
        std::fs::write(&path, bytes).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(checked_read(&base, &key).unwrap(), bytes);
        let getter = pinned_token(base.clone(), key.clone(), "declared-test-token".into());
        assert_eq!(getter().as_deref(), Some("declared-test-token"));
        assert!(checked_read(&base, "../escape").is_err());
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(checked_read(&base, &key).is_err());
        assert!(getter().is_none());
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert!(checked_read(&base, &key).is_ok());
        assert!(
            getter().is_none(),
            "observed credential loss must remain fenced"
        );
        std::fs::write(&path, b"changed").unwrap();
        assert!(checked_read(&base, &key).is_err());
        std::fs::remove_file(&path).unwrap();
        std::os::unix::fs::symlink("elsewhere", &path).unwrap();
        assert!(checked_read(&base, &key).is_err());
        std::fs::remove_dir_all(base).unwrap();
    }
    #[test]
    fn ffi_private_entry_refuses_unbound_invocation_before_worker() {
        let root = std::env::temp_dir().join(format!(
            "selected-ffi-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let runtime = crate::TrollmRuntime::new(root.to_string_lossy().into(), None).unwrap();
        let base = root.join("trollm");
        let parent = base.join("selected-installations");
        std::fs::create_dir(&parent).unwrap();
        std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o700)).unwrap();
        let body = serde_json::to_vec(&serde_json::json!({
            "schema":"trollm.private-selected-installation.v1",
            "installationSha256":"a".repeat(64),
            "ready":{"invocationKey":"original"},
            "guardUrl":"https://localhost/guard", "guardToken":"declared-test-token-only",
            "guardRootsPem":"not a certificate", "selectedDeviceMaxErrorUs":1,
            "expectedStaticKeySha256":"b".repeat(64),
        }))
        .unwrap();
        let key = crate::collection::raw_sha256(&body);
        let path = parent.join(format!("{key}.json"));
        std::fs::write(&path, body).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let error = runtime
            .start_selected_source_run_configured(
                "c".repeat(64),
                "a".repeat(64),
                "{}".into(),
                "{}".into(),
                "{}".into(),
                key.clone(),
                "different".into(),
            )
            .unwrap_err();
        assert!(
            matches!(error,crate::TrollmError::SelectedStart {code,native_run_id:None}
            if code=="SELECTED_INSTALLATION_LOAD")
        );
        let internal = load(&base, &key, &"a".repeat(64), "different")
            .err()
            .unwrap();
        assert!(internal.to_string().contains("source mismatch"));
        assert_eq!(std::fs::read_dir(base.join("runs")).unwrap().count(), 0);
        assert!(runtime
            .selected_run_binding("00000000-0000-4000-8000-000000000000".into())
            .is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
}
