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

/// Native-owned parsed tool envelope artifact, not original network bytes and
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
    let root = native_artifact_root.canonicalize()?;
    let actual = artifact.canonicalize()?;
    ensure!(
        actual.starts_with(&root) && actual == artifact,
        "selected result artifact path drift"
    );
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
    ensure!(
        bytes.len() <= 1_048_576
            && before.len() == bytes.len() as u64
            && opened.len() == after.len()
            && opened.mtime() == after.mtime()
            && opened.mtime_nsec() == after.mtime_nsec(),
        "selected result artifact drift"
    );
    let metadata_path = std::path::PathBuf::from(format!("{}.meta.json", artifact.display()));
    let metadata = std::fs::symlink_metadata(&metadata_path)?;
    ensure!(
        metadata.is_file() && !metadata.file_type().is_symlink() && metadata.len() <= 4096,
        "selected result completion metadata unavailable"
    );
    let complete: serde_json::Value =
        serde_json::from_reader(std::fs::File::open(&metadata_path)?.take(4097))?;
    ensure!(
        complete["complete"] == true,
        "selected result artifact incomplete"
    );
    let envelope: serde_json::Value = serde_json::from_slice(&bytes)?;
    ensure!(
        envelope == result.content_for_model && envelope.is_object(),
        "selected result envelope was shaped or mismatched"
    );
    ensure!(
        envelope.get("isError").is_none_or(|value| value == false),
        "selected tool error remains unresolved"
    );
    let value = if let Some(value) = envelope.get("structuredContent") {
        ensure!(value.is_object(), "selected structured result invalid");
        value.clone()
    } else {
        let content = envelope["content"]
            .as_array()
            .context("selected result content missing")?;
        let texts: Vec<_> = content
            .iter()
            .filter(|item| item["type"] == "text")
            .collect();
        ensure!(
            texts.len() == 1,
            "selected result requires one JSON text observation"
        );
        serde_json::from_str(
            texts[0]["text"]
                .as_str()
                .context("selected result text missing")?,
        )?
    };
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

#[cfg(test)]
mod relay_result_controls {
    use super::*;
    use serde_json::json;
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
        let observation = json!({"structuredContent":{"result":"mutation","actionClassification":"mutation-observed"}});
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
            let envelope = json!({"structuredContent":{"result":disposition,"actionClassification":classification}});
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
            json!({"structuredContent":{"result":"mutation"}}),
            json!({"structuredContent":{"result":"mutation","actionClassification":"navigation"}}),
            json!({"structuredContent":{"result":"dispatched-indeterminate"}}),
            json!({"structuredContent":{"result":"dispatched-unconfirmed"}}),
            json!({"structuredContent":{"result":"mutation","approvalRequired":true}}),
            json!({"structuredContent":{"result":"partial"}}),
            json!({"structuredContent":{"result":"navigation"}}),
            json!({"structuredContent":{"result":"navigation","actionClassification":"dispatched-unconfirmed"}}),
            json!({"isError":true,"structuredContent":{"result":"mutation"}}),
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
    let artifact = root.join(format!("{name}.json"));
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
    trollm_core::EventLog::visit(&journal, Some(&context.run_id), |event, raw| {
        if event["type"] == "artifact_written"
            && event["path"].as_str() == artifact.to_str()
            && event["seq"].as_u64().is_some_and(|seq| seq > sequences.1)
            && event["sha256"] == observed.sha256
            && event["bytes"].as_u64() == Some(observed.bytes.len() as u64)
        {
            artifact_record = Some(raw.to_owned());
        }
    })?;
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
        let artifact = artifacts.join("returned.json");
        let envelope = json!({"structuredContent":{"result":disposition,
            "actionClassification":if disposition == "navigation" { "navigation" } else { "mutation-observed" }}});
        let raw = serde_json::to_vec(&envelope).unwrap();
        std::fs::write(&artifact, &raw).unwrap();
        std::fs::write(
            artifacts.join("returned.json.meta.json"),
            b"{\"complete\":true}",
        )
        .unwrap();
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
        if accepted {
            let outcome = outcome.unwrap();
            assert_eq!(outcome["outcome"], "OBSERVATION_RETURNED");
            assert_eq!(outcome["contextSha256"], context.raw().unwrap().sha256);
        } else {
            assert!(
                outcome.is_err(),
                "mismatched native action/receipt must not acknowledge"
            );
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
    }
}
