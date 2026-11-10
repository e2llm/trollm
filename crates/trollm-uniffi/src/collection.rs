//! Bounded decoding of the immutable supplier selection, not owned admission.
//! Source: Toolbox eb0935cf1e1dc95f91e466df7ebdfc87cce226e0.
use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

pub const MAX_SELECTION_BYTES: usize = 65_536;
pub const SCHEMA: &str = "moray.declared-collection-selection.v1-proposal";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Owner {
    pub registration_id: String,
    pub tenant_id: String,
    pub subject: String,
    pub worker_id: String,
    pub run_id: String,
    pub epoch: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OwnedRef {
    pub tenant_id: String,
    pub artifact_id: String,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Source {
    pub source_ref: OwnedRef,
    pub profile_id: String,
    pub source_identity_sha256: String,
    pub dependency_refs: Vec<OwnedRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Item {
    pub original_item_id: String,
    pub input_position: u32,
    pub admitted_order: u32,
    pub quantity: u32,
    pub quantity_text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ParentOrigin {
    None,
    OfflineDeclared,
    ExecutedTarget,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Continuation {
    pub parent_checkpoint_ref: Option<OwnedRef>,
    pub parent_trial_ref: Option<OwnedRef>,
    pub known_committed_item_ids: Vec<String>,
    pub prefix_length: u32,
    pub parent_origin: ParentOrigin,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Variant {
    Original,
    Changed,
    Subset,
    Empty,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Selection {
    pub schema: String,
    pub owner: Owner,
    pub selection_id: String,
    pub case_id: String,
    pub invocation_key: String,
    pub variant: Variant,
    pub source: Source,
    pub application_ref: OwnedRef,
    pub requests_ref: OwnedRef,
    pub support_ref: OwnedRef,
    pub context_ref: OwnedRef,
    pub items: Vec<Item>,
    pub continuation: Continuation,
}

/// Preserve these original bytes. A decoded record establishes neither current
/// authorization nor source-profile identity, restoration or target admission.
pub struct SelectionRecord {
    pub selection: Selection,
    pub original_bytes: Vec<u8>,
    pub selection_ref: OwnedRef,
}

pub fn raw_sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

impl SelectionRecord {
    pub fn decode(bytes: &[u8], reference: &OwnedRef, expected_owner: &Owner) -> Result<Self> {
        ensure!(
            bytes.len() <= MAX_SELECTION_BYTES,
            "selection exceeds byte limit"
        );
        ensure!(
            reference.tenant_id == expected_owner.tenant_id,
            "selection tenant mismatch"
        );
        ensure!(
            reference
                .artifact_id
                .strip_prefix("art_")
                .is_some_and(|id| id.len() == 32
                    && id
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))),
            "invalid selection artifact reference"
        );
        ensure!(
            reference.sha256 == raw_sha256(bytes),
            "selection raw hash mismatch"
        );
        // Typed decoding refuses duplicate fields before any Value projection.
        let selection: Selection =
            serde_json::from_slice(bytes).context("invalid closed selection")?;
        ensure!(
            &selection.owner == expected_owner,
            "selection owner mismatch"
        );
        let value = serde_json::from_slice(bytes)?;
        trollm_mcp::ArtifactSchema::compile(&serde_json::from_str(include_str!(
            "../../../schemas/declared-collection-selection.v1.schema.json"
        ))?)?
        .validate(&value, MAX_SELECTION_BYTES)?;
        ensure!(
            trollm_mcp::preparation::js_sorted_json_bytes(&value)? == bytes,
            "selection bytes are not the producer canonical serialization"
        );
        ensure!(selection.schema == SCHEMA, "selection schema mismatch");
        selection.validate_relations()?;
        Ok(Self {
            selection,
            original_bytes: bytes.to_vec(),
            selection_ref: reference.clone(),
        })
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use serde_json::{json, Value};

    pub(crate) fn body() -> Value {
        let reference = json!({"tenantId":"test-tenant","artifactId":format!("art_{}", "a".repeat(32)),"sha256":"b".repeat(64)});
        json!({"schema":SCHEMA,"owner":{"registrationId":"test-registration","tenantId":"test-tenant",
            "subject":"test-subject","workerId":"test-worker","runId":"test-source","epoch":1},
            "selectionId":"test-original","caseId":"test-case","invocationKey":"test-invocation","variant":"original",
            "source":{"sourceRef":reference,"profileId":"declared-test-only","sourceIdentitySha256":"c".repeat(64),"dependencyRefs":[reference]},
            "applicationRef":reference,"requestsRef":reference,"supportRef":reference,"contextRef":reference,
            "items":[{"originalItemId":"x9","inputPosition":0,"admittedOrder":0,"quantity":1,"quantityText":"1"},
                {"originalItemId":"k2","inputPosition":1,"admittedOrder":1,"quantity":2,"quantityText":"2"},
                {"originalItemId":"z7","inputPosition":2,"admittedOrder":2,"quantity":3,"quantityText":"3"}],
            "continuation":{"parentCheckpointRef":null,"parentTrialRef":null,"knownCommittedItemIds":[],"prefixLength":0,"parentOrigin":"none"}})
    }

    pub(crate) fn decode(value: &Value) -> Result<SelectionRecord> {
        let bytes = trollm_mcp::preparation::js_sorted_json_bytes(value)?;
        let owner = serde_json::from_value(value["owner"].clone())?;
        let reference = OwnedRef {
            tenant_id: "test-tenant".into(),
            artifact_id: format!("art_{}", "d".repeat(32)),
            sha256: raw_sha256(&bytes),
        };
        SelectionRecord::decode(&bytes, &reference, &owner)
    }

    #[test]
    fn each_independent_original_changed_subset_empty_body_retains_its_bytes() {
        for variant in ["original", "changed", "subset", "empty"] {
            let mut b = body();
            b["variant"] = json!(variant);
            b["selectionId"] = json!(format!("test-{variant}"));
            if variant == "changed" {
                b["items"][0]["originalItemId"] = json!("changed-ID");
            }
            if variant == "subset" {
                b["items"] = json!([b["items"][2].clone()]);
                b["items"][0]["admittedOrder"] = json!(0);
            }
            if variant == "empty" {
                b["items"] = json!([]);
            }
            let record = decode(&b).unwrap();
            assert_eq!(
                record.original_bytes,
                trollm_mcp::preparation::js_sorted_json_bytes(&b).unwrap()
            );
            assert_eq!(
                record.selection_ref.sha256,
                raw_sha256(&record.original_bytes)
            );
        }
    }

    #[test]
    fn closed_selection_relations_and_raw_reference_drift_refuse() {
        for change in 0..8 {
            let mut b = body();
            match change {
                0 => b["items"][0]["quantityText"] = json!("2"),
                1 => b["items"][1]["originalItemId"] = b["items"][0]["originalItemId"].clone(),
                2 => b["items"][1]["inputPosition"] = json!(0),
                3 => b["items"][0]["admittedOrder"] = json!(1),
                4 => b["variant"] = json!("empty"),
                5 => b["applicationRef"]["tenantId"] = json!("other"),
                6 => b["continuation"]["knownCommittedItemIds"] = json!(["x9"]),
                _ => b["continuation"]["parentOrigin"] = json!("executed-target"),
            }
            assert!(decode(&b).is_err(), "mutation {change}");
        }
        let r = decode(&body()).unwrap();
        let mut bytes = r.original_bytes.clone();
        bytes.push(b' ');
        let mut reference = r.selection_ref.clone();
        reference.sha256 = raw_sha256(&bytes);
        assert!(SelectionRecord::decode(&bytes, &reference, &r.selection.owner).is_err());
        assert!(
            SelectionRecord::decode(&r.original_bytes, &reference, &r.selection.owner).is_err()
        );
        let duplicate = String::from_utf8(r.original_bytes.clone())
            .unwrap()
            .replacen("\"schema\":", "\"schema\":\"duplicate\",\"schema\":", 1);
        reference.sha256 = raw_sha256(duplicate.as_bytes());
        assert!(
            SelectionRecord::decode(duplicate.as_bytes(), &reference, &r.selection.owner).is_err()
        );
    }

    #[test]
    fn known_prefix_requires_matching_origin_and_retained_parent_refs() {
        let mut b = body();
        b["continuation"]["parentOrigin"] = json!("offline-declared");
        b["continuation"]["parentCheckpointRef"] = b["applicationRef"].clone();
        b["continuation"]["knownCommittedItemIds"] = json!(["x9", "k2"]);
        b["continuation"]["prefixLength"] = json!(2);
        decode(&b).unwrap();
        b["continuation"]["parentTrialRef"] = b["requestsRef"].clone();
        assert!(decode(&b).is_err());
        b["continuation"]["parentOrigin"] = json!("executed-target");
        decode(&b).unwrap();
        b["continuation"]["knownCommittedItemIds"] = json!(["k2", "x9"]);
        assert!(decode(&b).is_err());
    }
}

impl Selection {
    fn validate_relations(&self) -> Result<()> {
        let mut refs = vec![
            &self.source.source_ref,
            &self.application_ref,
            &self.requests_ref,
            &self.support_ref,
            &self.context_ref,
        ];
        refs.extend(&self.source.dependency_refs);
        refs.extend(self.continuation.parent_checkpoint_ref.iter());
        refs.extend(self.continuation.parent_trial_ref.iter());
        ensure!(
            refs.iter().all(|r| r.tenant_id == self.owner.tenant_id),
            "reference tenant mismatch"
        );
        ensure!(
            (self.variant == Variant::Empty) == self.items.is_empty(),
            "selection variant/item mismatch"
        );
        let mut ids = BTreeSet::new();
        let mut positions = BTreeSet::new();
        for (order, item) in self.items.iter().enumerate() {
            ensure!(
                item.admitted_order as usize == order,
                "admitted order is not contiguous"
            );
            ensure!(
                ids.insert(&item.original_item_id),
                "duplicate original item ID"
            );
            ensure!(
                positions.insert(item.input_position),
                "duplicate original input position"
            );
            ensure!(
                item.quantity_text == item.quantity.to_string(),
                "quantity text mismatch"
            );
        }
        let c = &self.continuation;
        ensure!(
            c.prefix_length as usize == c.known_committed_item_ids.len(),
            "prefix length mismatch"
        );
        ensure!(
            c.known_committed_item_ids.len() <= self.items.len(),
            "prefix exceeds selection"
        );
        ensure!(
            c.known_committed_item_ids
                .iter()
                .zip(&self.items)
                .all(|(id, item)| id == &item.original_item_id),
            "committed prefix mismatch"
        );
        match c.parent_origin {
            ParentOrigin::None => ensure!(
                c.parent_checkpoint_ref.is_none()
                    && c.parent_trial_ref.is_none()
                    && c.prefix_length == 0,
                "parent-free selection has continuation"
            ),
            ParentOrigin::OfflineDeclared => ensure!(
                c.parent_checkpoint_ref.is_some() && c.parent_trial_ref.is_none(),
                "offline parent reference mismatch"
            ),
            ParentOrigin::ExecutedTarget => ensure!(
                c.parent_checkpoint_ref.is_some() && c.parent_trial_ref.is_some(),
                "executed parent references missing"
            ),
        }
        Ok(())
    }
}

/// Native-owned parsed tool payload artifact, not original network bytes and
/// never independent cart/business verification. The caller must separately
/// bind its hash/path to the retained native journal and installed authority.
pub struct RetainedRelayObservation {
    pub bytes: Vec<u8>,
    pub sha256: String,
    pub disposition: String,
}

pub fn retained_relay_observation(
    result: &trollm_mcp::ToolResult,
    artifact: &std::path::Path,
    native_artifact_root: &std::path::Path,
) -> Result<RetainedRelayObservation> {
    use std::io::Read;
    ensure!(
        result.provenance == trollm_mcp::Provenance::Live,
        "selected effect result must be live, never cache or supplied fixture"
    );
    ensure!(
        artifact.is_absolute()
            && native_artifact_root.is_absolute()
            && result
                .artifacts
                .iter()
                .filter(|path| path.as_path() == artifact)
                .count()
                == 1,
        "selected result artifact association unavailable"
    );
    let root_metadata = std::fs::symlink_metadata(native_artifact_root)?;
    for directory in [
        native_artifact_root.parent(),
        native_artifact_root
            .parent()
            .and_then(std::path::Path::parent),
    ] {
        let metadata =
            std::fs::symlink_metadata(directory.context("selected result native root missing")?)?;
        ensure!(
            metadata.is_dir() && !metadata.file_type().is_symlink(),
            "selected result native directory alias refused"
        );
    }
    ensure!(
        root_metadata.is_dir() && !root_metadata.file_type().is_symlink(),
        "selected result artifact root type refused"
    );
    let root = native_artifact_root.canonicalize()?;
    let actual_result = artifact.canonicalize();
    let relative = artifact.strip_prefix(native_artifact_root)?;
    // Only the installed policy's one immediate regular artifact is selected.
    // A trusted app-root spelling may be an OS alias; an alias *inside* this
    // native artifact directory must never select another file or directory.
    let immediate = relative.components().count() == 1
        && matches!(
            relative.components().next(),
            Some(std::path::Component::Normal(_))
        );
    let expected = root.join(relative);
    let confined = immediate
        && actual_result
            .as_ref()
            .is_ok_and(|actual| *actual == expected);
    let diagnostic = serde_json::json!({
        "schema":"trollm.selected-result-path-diagnostic.v1",
        "scope":"original-local-path-operands-not-outcome-or-settlement",
        "logicalArtifactRoot":native_artifact_root,"logicalArtifact":artifact,
        "canonicalRoot":root,"canonicalArtifact":actual_result.as_ref().ok(),
        "canonicalArtifactError":actual_result.as_ref().err().map(ToString::to_string),
        "expectedCanonicalArtifact":expected,"immediateMember":immediate,"confined":confined
    });
    let diagnostic_result = retain_result_path_diagnostic(&root, &diagnostic);
    ensure!(confined, "selected result artifact path drift");
    diagnostic_result.context("selected result path diagnostic unavailable")?;
    let before = std::fs::symlink_metadata(artifact)?;
    ensure!(
        before.is_file() && !before.file_type().is_symlink() && before.len() <= 1_048_576,
        "selected result artifact type or bound refused"
    );
    let mut file = std::fs::File::open(artifact)?;
    let opened = file.metadata()?;
    use std::os::unix::fs::MetadataExt;
    ensure!(
        before.dev() == opened.dev() && before.ino() == opened.ino(),
        "selected result artifact replaced during open"
    );
    let mut bytes = Vec::new();
    file.by_ref().take(1_048_577).read_to_end(&mut bytes)?;
    let after = file.metadata()?;
    let named = std::fs::symlink_metadata(artifact)?;
    let root_after = std::fs::symlink_metadata(native_artifact_root)?;
    ensure!(
        bytes.len() <= 1_048_576
            && before.len() == bytes.len() as u64
            && opened.len() == after.len()
            && opened.mtime() == after.mtime()
            && opened.mtime_nsec() == after.mtime_nsec()
            && before.dev() == named.dev()
            && before.ino() == named.ino()
            && named.is_file()
            && !named.file_type().is_symlink()
            && named.len() == after.len()
            && named.mtime() == after.mtime()
            && named.mtime_nsec() == after.mtime_nsec()
            && root_after.is_dir()
            && !root_after.file_type().is_symlink()
            && root_metadata.dev() == root_after.dev()
            && root_metadata.ino() == root_after.ino()
            && native_artifact_root.canonicalize()? == root
            && artifact.canonicalize()? == expected,
        "selected result artifact drift"
    );
    let metadata_path = std::path::PathBuf::from(format!("{}.meta.json", artifact.display()));
    let metadata = std::fs::symlink_metadata(&metadata_path)?;
    ensure!(
        metadata.is_file() && !metadata.file_type().is_symlink() && metadata.len() <= 4096,
        "selected result completion metadata unavailable"
    );
    let mut metadata_file = std::fs::File::open(&metadata_path)?;
    let metadata_opened = metadata_file.metadata()?;
    ensure!(
        metadata.dev() == metadata_opened.dev() && metadata.ino() == metadata_opened.ino(),
        "selected result completion metadata replaced during open"
    );
    let mut metadata_bytes = Vec::new();
    metadata_file
        .by_ref()
        .take(4097)
        .read_to_end(&mut metadata_bytes)?;
    let metadata_after = metadata_file.metadata()?;
    let metadata_named = std::fs::symlink_metadata(&metadata_path)?;
    ensure!(
        metadata_bytes.len() <= 4096
            && metadata_bytes.len() as u64 == metadata.len()
            && metadata_after.len() == metadata.len()
            && metadata_named.len() == metadata.len()
            && metadata_named.is_file()
            && !metadata_named.file_type().is_symlink()
            && metadata_named.dev() == metadata.dev()
            && metadata_named.ino() == metadata.ino()
            && metadata_after.mtime() == metadata.mtime()
            && metadata_named.mtime() == metadata.mtime()
            && metadata_after.mtime_nsec() == metadata.mtime_nsec()
            && metadata_named.mtime_nsec() == metadata.mtime_nsec(),
        "selected result completion metadata drift"
    );
    let complete: serde_json::Value = serde_json::from_slice(&metadata_bytes)?;
    ensure!(
        complete["complete"] == true,
        "selected result artifact incomplete"
    );
    let value: serde_json::Value = serde_json::from_slice(&bytes)?;
    ensure!(
        value == result.content_for_model && value.is_object(),
        "selected result payload was shaped or mismatched"
    );
    ensure!(
        value.get("isError").is_none_or(|value| value == false),
        "selected tool error remains unresolved"
    );
    // McpGateway::finish_result persists extract_tool_payload's output. A
    // second MCP unwrap would reject the actual producer or reinterpret a
    // payload field as transport authority. Preserve these exact bytes.
    ensure!(
        !value
            .get("approvalRequired")
            .is_some_and(|value| value != false),
        "selected preview is not a returned effect observation"
    );
    let disposition = value["result"]
        .as_str()
        .context("selected disposition missing")?;
    ensure!(
        ["mutation", "recapture", "no-change", "navigation"].contains(&disposition),
        "selected uncertain, partial or unrecognized disposition"
    );
    let classification = match disposition {
        "mutation" => "mutation-observed",
        "recapture" => "recapture-triggered",
        "no-change" => "dispatched-no-dom-change",
        "navigation" => "navigation",
        _ => unreachable!("selected disposition was validated above"),
    };
    ensure!(
        value["actionClassification"] == classification,
        "selected returned action classification missing or mismatched"
    );
    Ok(RetainedRelayObservation {
        sha256: format!("{:x}", Sha256::digest(&bytes)),
        bytes,
        disposition: disposition.into(),
    })
}

fn retain_result_path_diagnostic(root: &std::path::Path, value: &serde_json::Value) -> Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let bytes = serde_json::to_vec(value)?;
    ensure!(
        bytes.len() <= 65_536,
        "selected result path diagnostic bound"
    );
    let path = root.join(format!("native-result-path-{}.json", raw_sha256(&bytes)));
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)
    {
        Ok(mut file) => {
            file.write_all(&bytes)?;
            file.sync_all()?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            let metadata = std::fs::symlink_metadata(&path)?;
            ensure!(
                metadata.is_file()
                    && !metadata.file_type().is_symlink()
                    && metadata.len() == bytes.len() as u64
                    && std::fs::read(&path)? == bytes,
                "selected result path diagnostic conflict"
            );
        }
        Err(error) => return Err(error.into()),
    }
    std::fs::File::open(root)?.sync_all()?;
    Ok(())
}

#[cfg(test)]
mod relay_result_controls {
    use super::*;
    use serde_json::json;
    #[test]
    fn original_app_root_alias_is_distinct_from_in_run_symlink_or_foreign_file() {
        let parent = std::env::temp_dir().join(format!(
            "selected-path-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let base = parent.join("real-app");
        let alias = parent.join("app-prefix-alias");
        let root = base.join("runs/original-run/artifacts");
        std::fs::create_dir_all(&root).unwrap();
        std::os::unix::fs::symlink(&base, &alias).unwrap();
        let logical_root = alias.join("runs/original-run/artifacts");
        let artifact = logical_root.join("actual.json");
        let payload = json!({"result":"mutation","actionClassification":"mutation-observed"});
        let raw = serde_json::to_vec(&payload).unwrap();
        std::fs::write(&artifact, &raw).unwrap();
        std::fs::write(
            format!("{}.meta.json", artifact.display()),
            b"{\"complete\":true}",
        )
        .unwrap();
        let result = trollm_mcp::ToolResult {
            content_for_model: payload,
            artifacts: vec![artifact.clone()],
            provenance: trollm_mcp::Provenance::Live,
        };
        assert_ne!(artifact.canonicalize().unwrap(), artifact);
        assert_eq!(
            retained_relay_observation(&result, &artifact, &logical_root)
                .unwrap()
                .bytes,
            raw
        );
        let diagnostic = std::fs::read_dir(&root)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .find(|path| {
                path.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("native-result-path-")
            })
            .unwrap();
        let bytes = std::fs::read(&diagnostic).unwrap();
        let operands: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(operands["logicalArtifact"], artifact.to_str().unwrap());
        assert_eq!(
            operands["canonicalArtifact"],
            root.join("actual.json").to_str().unwrap()
        );
        assert_eq!(operands["confined"], true);
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(diagnostic).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let linked = logical_root.join("linked.json");
        std::os::unix::fs::symlink(&artifact, &linked).unwrap();
        let mut foreign = result.clone();
        foreign.artifacts = vec![linked.clone()];
        assert!(retained_relay_observation(&foreign, &linked, &logical_root).is_err());
        std::fs::remove_file(format!("{}.meta.json", artifact.display())).unwrap();
        std::os::unix::fs::symlink(&artifact, format!("{}.meta.json", artifact.display())).unwrap();
        assert!(retained_relay_observation(&result, &artifact, &logical_root).is_err());
        std::fs::remove_dir_all(parent).unwrap();
    }
    #[test]
    fn retained_live_artifact_refuses_uncertain_preview_shape_cache_and_symlink() {
        let root = std::env::temp_dir().join(format!(
            "selected-relay-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        let artifact = root.join("actual.json");
        let meta = root.join("actual.json.meta.json");
        std::fs::write(&meta, b"{\"complete\":true}").unwrap();
        let observation = json!({"result":"mutation","actionClassification":"mutation-observed"});
        let result = trollm_mcp::ToolResult {
            content_for_model: observation.clone(),
            artifacts: vec![artifact.clone()],
            provenance: trollm_mcp::Provenance::Live,
        };
        let bytes = serde_json::to_vec(&observation).unwrap();
        std::fs::write(&artifact, &bytes).unwrap();
        let retained = retained_relay_observation(&result, &artifact, &root).unwrap();
        assert_eq!(retained.bytes, bytes);
        assert_eq!(retained.disposition, "mutation");
        for (disposition, classification) in [
            ("mutation", "mutation-observed"),
            ("recapture", "recapture-triggered"),
            ("no-change", "dispatched-no-dom-change"),
            ("navigation", "navigation"),
        ] {
            let envelope = json!({"result":disposition,"actionClassification":classification});
            std::fs::write(&artifact, serde_json::to_vec(&envelope).unwrap()).unwrap();
            let mut returned = result.clone();
            returned.content_for_model = envelope;
            assert_eq!(
                retained_relay_observation(&returned, &artifact, &root)
                    .unwrap()
                    .disposition,
                disposition
            );
        }
        for value in [
            json!({"result":"mutation"}),
            json!({"result":"mutation","actionClassification":"navigation"}),
            json!({"result":"dispatched-indeterminate"}),
            json!({"result":"dispatched-unconfirmed"}),
            json!({"result":"mutation","approvalRequired":true}),
            json!({"result":"partial"}),
            json!({"result":"navigation"}),
            json!({"result":"navigation","actionClassification":"dispatched-unconfirmed"}),
            json!({"isError":true,"result":"mutation","actionClassification":"mutation-observed"}),
            json!({"structuredContent":{"result":"mutation","actionClassification":"mutation-observed"}}),
        ] {
            std::fs::write(&artifact, serde_json::to_vec(&value).unwrap()).unwrap();
            let mut returned = result.clone();
            returned.content_for_model = value;
            assert!(retained_relay_observation(&returned, &artifact, &root).is_err());
        }
        std::fs::write(&artifact, &bytes).unwrap();
        let mut cached = result.clone();
        cached.provenance = trollm_mcp::Provenance::Cache;
        assert!(retained_relay_observation(&cached, &artifact, &root).is_err());
        let mut shaped = result.clone();
        shaped.content_for_model = json!({"result":"mutation"});
        assert!(retained_relay_observation(&shaped, &artifact, &root).is_err());
        std::fs::write(&meta, b"{\"complete\":false}").unwrap();
        assert!(retained_relay_observation(&result, &artifact, &root).is_err());
        std::fs::write(&meta, b"{\"complete\":true}").unwrap();
        let link = root.join("linked.json");
        std::os::unix::fs::symlink(&artifact, &link).unwrap();
        let mut linked = result.clone();
        linked.artifacts = vec![link.clone()];
        assert!(retained_relay_observation(&linked, &link, &root).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
}

/// Build the literal selected outcome only from a native-owned journal and its
/// retained live artifact. This generated association envelope labels embedded
/// original record text explicitly; it is not original JSONL/HTTP/kernel bytes.
/// Installation/current-authority and stopped-worker checks remain separate.
pub fn retained_outcome_request(
    selection: &SelectionRecord,
    context: &trollm_mcp::preparation::PreparationContext,
    call: &trollm_mcp::ToolCall,
    result: &trollm_mcp::ToolResult,
    run_directory: &std::path::Path,
    sequences: (u64, u64),
    installed_policy_artifact: &str,
) -> Result<serde_json::Value> {
    use base64::Engine;
    use serde_json::json;
    ensure!(
        run_directory.is_absolute()
            && run_directory.file_name().and_then(|v| v.to_str()) == Some(context.run_id.as_str()),
        "native outcome run directory mismatch"
    );
    ensure!(
        selection.selection_ref.sha256 == raw_sha256(&selection.original_bytes),
        "native outcome selection drift"
    );
    let original = SelectionRecord::decode(
        &selection.original_bytes,
        &selection.selection_ref,
        &selection.selection.owner,
    )?;
    ensure!(
        serde_json::to_value(&original.selection)? == serde_json::to_value(&selection.selection)?,
        "native outcome selection body association drift"
    );
    let journal = run_directory.join("events.jsonl");
    let records = trollm_core::events::retained_call_records(
        &journal,
        context,
        call,
        sequences.0,
        sequences.1,
    )?;
    let result_event: serde_json::Value = serde_json::from_str(&records.result_json)?;
    // Task-level resultArtifact remains a remote-read-only contract. Selected
    // mutations retain the installed tool policy's exact native destination.
    let name = installed_policy_artifact;
    ensure!(
        !name.is_empty()
            && name.len() <= 128
            && name
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-')),
        "selected resultArtifact name unsupported"
    );
    let root = run_directory.join("artifacts");
    let artifact = root.join(trollm_mcp::preparation::selected_result_artifact_name(
        name, context,
    )?);
    ensure!(
        result_event["artifacts"]
            .as_array()
            .is_some_and(|paths| paths
                .iter()
                .filter(|path| path.as_str() == artifact.to_str())
                .count()
                == 1),
        "native journal result artifact mismatch"
    );
    let observed = retained_relay_observation(result, &artifact, &root)?;
    ensure!(
        (call.args["action"] == "navigate") == (observed.disposition == "navigation"),
        "retained navigation receipt does not match original sent action"
    );
    let mut artifact_record = None;
    let mut conflicting_artifact = false;
    trollm_core::EventLog::visit(&journal, Some(&context.run_id), |event, raw| {
        if event["type"] == "artifact_written" && event["path"].as_str() == artifact.to_str() {
            if event["seq"].as_u64().is_some_and(|seq| seq > sequences.1)
                && event["sha256"] == observed.sha256
                && event["bytes"].as_u64() == Some(observed.bytes.len() as u64)
            {
                artifact_record = Some(raw.to_owned());
            } else {
                conflicting_artifact = true;
            }
        }
    })?;
    ensure!(
        !conflicting_artifact,
        "native retained artifact commitment conflict"
    );
    let artifact_record =
        artifact_record.context("native retained artifact hash association unavailable")?;
    let evidence = serde_json::to_vec(&json!({
        "schema":"trollm.selected-native-outcome-association.v1",
        "encoding":"native-generated-association-with-original-json-record-text",
        "contextSha256":context.raw()?.sha256,
        "intentRecordJson":records.intent_json,"resultRecordJson":records.result_json,
        "artifactRecordJson":artifact_record,
        "parsedEnvelopeArtifact":{"sha256":observed.sha256,"sizeBytes":observed.bytes.len(),
            "bytesBase64":base64::engine::general_purpose::STANDARD.encode(&observed.bytes)},
        "observationDisposition":observed.disposition,
    }))?;
    ensure!(
        evidence.len() <= 16384,
        "selected native outcome evidence exceeds wire bound"
    );
    Ok(
        json!({"kind":"outcome","selectionRef":selection.selection_ref,
        "selectionRawSha256":raw_sha256(&selection.original_bytes),"owner":selection.selection.owner,
        "selectionId":selection.selection.selection_id,"caseId":selection.selection.case_id,
        "invocationKey":selection.selection.invocation_key,"contextSha256":context.raw()?.sha256,
        "outcome":"OBSERVATION_RETURNED","journalEvidence":{
            "bytesBase64":base64::engine::general_purpose::STANDARD.encode(&evidence),
            "sha256":raw_sha256(&evidence)}}),
    )
}

#[cfg(test)]
mod outcome_association_controls {
    use super::*;
    use serde_json::json;
    #[test]
    fn original_journal_artifact_and_outcome_link_then_drift_refuses() {
        for (action, disposition, accepted) in [
            ("click", "mutation", true),
            ("navigate", "navigation", true),
            ("click", "navigation", false),
            ("navigate", "mutation", false),
        ] {
            check_original_journal(action, disposition, accepted);
        }
    }
    fn check_original_journal(action: &str, disposition: &str, accepted: bool) {
        let _ = check_original_journal_size(action, disposition, accepted, 0);
    }
    #[test]
    fn original_artifact_is_not_truncated_to_fit_outcome_evidence() {
        // Measure the actual native association representation, including
        // original journal record text, before choosing adjacent bounded cases.
        let base = check_original_journal_size("click", "mutation", true, 0).unwrap();
        let below = (16_384 - base - 64) * 3 / 4;
        let size = check_original_journal_size("click", "mutation", true, below).unwrap();
        assert!((16_200..=16_384).contains(&size));
        assert!(check_original_journal_size("click", "mutation", false, below + 256).is_none());
    }
    fn check_original_journal_size(
        action: &str,
        disposition: &str,
        accepted: bool,
        padding: usize,
    ) -> Option<usize> {
        let parent = std::env::temp_dir().join(format!(
            "native-outcome-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let run = parent.join("actual-run");
        let artifacts = run.join("artifacts");
        std::fs::create_dir_all(&artifacts).unwrap();
        let envelope = json!({"result":disposition,"padding":"x".repeat(padding),
            "actionClassification":if disposition == "navigation" { "navigation" } else { "mutation-observed" }});
        let raw = serde_json::to_vec(&envelope).unwrap();
        let selection = super::tests::decode(&super::tests::body()).unwrap();
        let context = trollm_mcp::preparation::PreparationContext {
            run_id: "actual-run".into(),
            node: "node".into(),
            task_ref: "task".into(),
            model_slot: "coder".into(),
            model_call_ordinal: 1,
            frames: vec![],
            cursors: vec![],
            argument_bindings: vec![],
            call_context: trollm_mcp::CallContext {
                epoch: "original".into(),
                attempt: u64::MAX,
                call: 1,
            },
        };
        let artifact = artifacts.join(
            trollm_mcp::preparation::selected_result_artifact_name("returned", &context).unwrap(),
        );
        std::fs::write(&artifact, &raw).unwrap();
        std::fs::write(
            format!("{}.meta.json", artifact.display()),
            b"{\"complete\":true}",
        )
        .unwrap();
        let call = trollm_mcp::ToolCall {
            server: "fixture".into(),
            tool: "act".into(),
            args: json!({"action":action,"target":"node-1"}),
        };
        let result = trollm_mcp::ToolResult {
            content_for_model: envelope,
            artifacts: vec![artifact.clone()],
            provenance: trollm_mcp::Provenance::Live,
        };
        let mut log = trollm_core::EventLog::open(&run.join("events.jsonl"), "actual-run").unwrap();
        let fields = json!({"node":"node","taskRef":"task","server":"fixture","tool":"act","callContext":context.call_context});
        let mut intent = fields.clone();
        intent["args"] = call.args.clone();
        let i = log
            .append(trollm_core::EventType::ToolCallIntent, intent)
            .unwrap();
        let mut returned = fields;
        returned["intentSeq"] = json!(i);
        returned["resultArtifact"] = serde_json::Value::Null; // Mutation uses policy artifact, not read-only task option.
        returned["artifacts"] = json!([artifact]);
        returned["argsHash"] = json!(trollm_mcp::CacheStore::hash_args(&call.args, None, None));
        let r = log
            .append(trollm_core::EventType::ToolCallResult, returned)
            .unwrap();
        assert!(
            retained_outcome_request(
                &selection,
                &context,
                &call,
                &result,
                &run,
                (i, r),
                "returned"
            )
            .is_err(),
            "artifact custody must precede acknowledgement"
        );
        log.append(
            trollm_core::EventType::ArtifactWritten,
            json!({"path":artifact,"bytes":raw.len(),"sha256":raw_sha256(&raw)}),
        )
        .unwrap();
        let outcome = retained_outcome_request(
            &selection,
            &context,
            &call,
            &result,
            &run,
            (i, r),
            "returned",
        );
        let retained_size = if accepted {
            let outcome = outcome.unwrap();
            assert_eq!(outcome["outcome"], "OBSERVATION_RETURNED");
            assert_eq!(outcome["contextSha256"], context.raw().unwrap().sha256);
            use base64::Engine;
            let evidence = base64::engine::general_purpose::STANDARD
                .decode(outcome["journalEvidence"]["bytesBase64"].as_str().unwrap())
                .unwrap();
            assert_eq!(outcome["journalEvidence"]["sha256"], raw_sha256(&evidence));
            let decoded: serde_json::Value = serde_json::from_slice(&evidence).unwrap();
            assert_eq!(
                base64::engine::general_purpose::STANDARD
                    .decode(
                        decoded["parsedEnvelopeArtifact"]["bytesBase64"]
                            .as_str()
                            .unwrap()
                    )
                    .unwrap(),
                raw
            );
            Some(evidence.len())
        } else {
            if padding > 0 {
                assert!(outcome
                    .as_ref()
                    .unwrap_err()
                    .to_string()
                    .contains("exceeds wire bound"));
            }
            assert!(
                outcome.is_err(),
                "mismatched native action/receipt must not acknowledge"
            );
            None
        };
        assert_eq!(
            std::fs::read(&artifact).unwrap(),
            raw,
            "refusal must retain the whole original artifact, not a fitted projection"
        );
        if accepted && padding == 0 {
            log.append(
                trollm_core::EventType::ArtifactWritten,
                json!({"path":artifact,"bytes":raw.len(),"sha256":"f".repeat(64)}),
            )
            .unwrap();
            assert!(retained_outcome_request(
                &selection,
                &context,
                &call,
                &result,
                &run,
                (i, r),
                "returned"
            )
            .unwrap_err()
            .to_string()
            .contains("commitment conflict"));
            assert_eq!(std::fs::read(&artifact).unwrap(), raw);
        }
        std::fs::write(&artifact, b"{}").unwrap();
        assert!(retained_outcome_request(
            &selection,
            &context,
            &call,
            &result,
            &run,
            (i, r),
            "returned"
        )
        .is_err());
        drop(log);
        std::fs::remove_dir_all(parent).unwrap();
        retained_size
    }
}
