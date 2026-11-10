//! Native progress evidence, never model-text or website verification.
use crate::{bundle, RunPhaseApi, RunStatusInfo};
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::{
    fs,
    path::{Component, Path},
};
use trollm_core::{artifacts::write_atomic, plan::hex_sha256, RunState};

/// Read original cursor bytes by their original path and last journal commitment.
/// No filename substitution, regeneration or missing-artifact recovery occurs.
fn read_committed_cursor_snapshot(
    dir: &Path,
    snapshot_ref: &str,
    committed: &std::collections::BTreeMap<String, (String, u64)>,
    conflicting: &std::collections::BTreeSet<String>,
) -> Result<(Vec<u8>, Vec<Value>)> {
    let relative = Path::new(snapshot_ref);
    if snapshot_ref.is_empty()
        || relative
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        bail!("invalid native snapshot reference");
    }
    let logical_path = dir.join(relative);
    if conflicting.contains(logical_path.to_string_lossy().as_ref()) {
        bail!("native item snapshot has conflicting immutable commitments");
    }
    let path = logical_path.canonicalize()?;
    if !path.starts_with(dir.canonicalize()?) || !fs::symlink_metadata(&logical_path)?.is_file() {
        bail!("native snapshot escapes run custody");
    }
    if fs::metadata(&path)?.len() > 32 * 1024 * 1024 {
        bail!("native snapshot report limit");
    }
    let raw = fs::read(&path)?;
    if raw.len() > 32 * 1024 * 1024 {
        bail!("native snapshot report limit");
    }
    let hash = hex_sha256(&raw);
    if !committed
        .get(logical_path.to_string_lossy().as_ref())
        .is_some_and(|(sha, size)| sha == &hash && *size == raw.len() as u64)
    {
        bail!("native item snapshot differs from its committed artifact identity");
    }
    let values = std::str::from_utf8(&raw)?
        .lines()
        .filter(|line| !line.is_empty())
        .map(serde_json::from_str)
        .collect::<std::result::Result<_, _>>()?;
    Ok((raw, values))
}

pub(crate) fn retain(
    base: &Path,
    run: &str,
    device: &str,
    source: &str,
    status: &RunStatusInfo,
) -> Result<String> {
    if status.worker_active {
        bail!("native worker still active");
    }
    if device.is_empty()
        || device.len() > 128
        || !device
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        bail!("invalid native device identity");
    }
    let association: Value = serde_json::from_str(source)?;
    let dir = base.join("runs").join(run);
    if fs::metadata(dir.join("state.snapshot.json"))?.len() > 32 * 1024 * 1024 {
        bail!("native state report limit");
    }
    let snapshot = fs::read(dir.join("state.snapshot.json"))
        .context("native snapshot unavailable for retained report")?;
    let state: RunState = serde_json::from_slice(&snapshot)?;
    if state.run_id != run || status.run_id != run {
        bail!("native report subject mismatch");
    }
    let revision = association["installation"]["installationSha256"]
        .as_str()
        .context("missing native revision")?;
    let installation = bundle::revision_dir(base, revision)?;
    let manifest = serde_json::from_slice(&fs::read(installation.join(bundle::MANIFEST_NAME))?)?;
    let (plan, _) = bundle::validate_materialized_bundle(&installation, &manifest)?;
    let mut items = Vec::new();
    let mut mappings = Vec::new();
    let mut retained = Vec::new();
    let snapshot_hash = hex_sha256(&snapshot);
    let mut committed_artifacts = std::collections::BTreeMap::new();
    let mut conflicting_artifacts = std::collections::BTreeSet::new();
    let journal_path = dir.join("events.jsonl");
    if fs::metadata(&journal_path)?.len() > 32 * 1024 * 1024 {
        bail!("native journal report limit");
    }
    let journal = fs::read(&journal_path)?;
    let journal_hash = hex_sha256(&journal);
    let last_seq = trollm_core::EventLog::visit(&journal_path, Some(run), |event, _| {
        let committed = event["seq"]
            .as_u64()
            .is_some_and(|seq| seq <= state.event_watermark)
            && event["type"] == "artifact_written";
        if let (Some(path), Some(sha), Some(bytes), true) = (
            event["path"].as_str(),
            event["sha256"].as_str(),
            event["bytes"].as_u64(),
            committed,
        ) {
            let next = (sha.to_string(), bytes);
            if committed_artifacts
                .get(path)
                .is_some_and(|previous| previous != &next)
            {
                conflicting_artifacts.insert(path.to_string());
            }
            committed_artifacts.insert(path.to_string(), next);
        }
    })?;
    if last_seq == 0
        || state.event_watermark > last_seq
        || last_seq != status.last_seq
        || fs::read(&journal_path)? != journal
    {
        bail!("native snapshot/journal observation changed or exceeds retained progress");
    }
    retained.push((format!("{journal_hash}.events.jsonl"), journal));
    let source_hash = hex_sha256(source.as_bytes());
    retained.push((
        format!("{source_hash}.source-association.json"),
        source.as_bytes().to_vec(),
    ));
    let root_cursors = state
        .stack
        .first()
        .map(|frame| &frame.foreach_cursors)
        .filter(|cursors| !cursors.is_empty())
        .unwrap_or(&state.foreach_cursors);
    for (node, cursor) in root_cursors {
        // Only root collections have a source association; do not guess child joins.
        let definition = plan
            .nodes
            .get(node)
            .context("nested native item report mapping unavailable")?;
        let key = definition
            .item_key
            .as_ref()
            .context("native item key unavailable")?;
        let (raw, values) = read_committed_cursor_snapshot(
            &dir,
            &cursor.snapshot_ref,
            &committed_artifacts,
            &conflicting_artifacts,
        )?;
        if items.len() + values.len() > 128
            || cursor.results.len() > values.len()
            || cursor.index > values.len()
        {
            bail!("native item report limit or inconsistent prefix");
        }
        let raw_hash = hex_sha256(&raw);
        retained.push((format!("{raw_hash}.jsonl"), raw));
        for (index, value) in values.iter().enumerate() {
            let original = value.get(key).context("native item identity missing")?;
            if original.is_null() {
                bail!("native item identity missing");
            }
            let classification = if let Some(result) = cursor.results.get(index) {
                if result["outcome"] == "succeeded" {
                    "observed"
                } else {
                    "outcome-unknown"
                }
            } else if index == cursor.index
                && matches!(cursor.phase, trollm_core::state::ForeachPhase::InBody)
            {
                "outcome-unknown"
            } else {
                "not-started"
            };
            let item_identity =
                hex_sha256(&serde_json::to_vec(&json!({"run":run,"plan":plan.sha256,
                "node":node,"inputIndex":index,"itemKey":key,"originalItemIdentity":original}))?);
            let mapping = json!({"nativeRunId":run,"plan":plan.sha256,"node":node,"inputIndex":index,
                "itemKey":key,"originalItemIdentity":original,"item":value,"classification":classification,
                "meaning":"native-progress-only","businessVerification":"not-assessed","snapshotSha256":raw_hash});
            let bytes = serde_json::to_vec(&mapping)?;
            let digest = hex_sha256(&bytes);
            items.push(
                json!({"itemId":format!("native-{item_identity}"),"classification":classification,
                "evidenceSha256":[digest,raw_hash,snapshot_hash]}),
            );
            mappings.push(mapping);
            retained.push((format!("{digest}.json"), bytes));
        }
    }
    let admission = if let Some(expected) = association.get("inputAdmission") {
        let raw = crate::setup::read_input_evidence(&dir, "input.original.json", 65536)?;
        let normalized = crate::setup::read_input_evidence(&dir, "input.normalized.json", 65536)?;
        if raw.len() > 65536 || normalized.len() > 65536 {
            bail!("input evidence exceeds bound");
        }
        let input = bundle::revision_setup(base, revision)?
            .and_then(|setup| setup.input)
            .context("input admission setup unavailable")?;
        let actual = crate::setup::admit_source_input(
            &input,
            std::str::from_utf8(&raw)?,
            std::str::from_utf8(&normalized)?,
        )?;
        let sealed = crate::setup::read_input_evidence(&dir, "input-admission.json", 524288)?;
        if &actual != expected || serde_json::from_slice::<Value>(&sealed)? != actual {
            bail!("input admission evidence mismatch");
        }
        for (kind, bytes) in [
            ("original", raw),
            ("normalized", normalized),
            ("admission", sealed),
        ] {
            retained.push((format!("{}.input-{kind}", hex_sha256(&bytes)), bytes));
        }
        Some(actual)
    } else {
        None
    };
    let mut observation = json!({"schema":"trollm.native-source-observation.v1","source":association,
        "nativeStatus":match &status.phase { RunPhaseApi::Succeeded=>"succeeded", RunPhaseApi::Failed{..}=>"failed", _=>"unknown" },
        "workerActive":false,"snapshotStatus":state.status,
        "progressDisposition":if matches!(state.status, trollm_core::state::StateStatus::Running | trollm_core::state::StateStatus::Paused) {"interrupted"} else {"terminal"},
        "journalSha256":journal_hash,"sourceAssociationSha256":source_hash,
        "nativeFailureReason":match &status.phase { RunPhaseApi::Failed{reason}=>Some(reason.as_str()),_=>None },
        "terminalReason":state.terminal_reason,"eventWatermark":state.event_watermark,"counters":state.counters,
        "snapshotSha256":snapshot_hash,"items":mappings,"businessVerification":"not-assessed"});
    if let Some(admission) = admission {
        observation["inputAdmission"] = admission;
    }
    let cause_path = dir.join("cancellation-cause.json");
    match fs::metadata(&cause_path) {
        Ok(metadata) => {
            if metadata.len() > 4096 {
                bail!("native cancellation cause exceeds bound");
            }
            let raw = fs::read(&cause_path)?;
            let cause: Value = serde_json::from_slice(&raw)?;
            let valid = [
                "OPERATOR_CANCELLED",
                "SELECTED_ACTOR_EXPIRED",
                "SELECTED_CLOCK_STALE",
                "SELECTED_CLOCK_DEADLINE",
                "SELECTED_CLOCK_DISCONTINUITY",
                "SELECTED_CLOCK_SOURCE_UNAVAILABLE",
                "SELECTED_CLOCK_RETIRED",
                "SELECTED_CLOCK_UNAVAILABLE",
                "SELECTED_CLOCK_REFRESH_TIMEOUT",
            ];
            if cause["schema"] != "trollm.native-cancellation-cause.v1-proposal"
                || cause["nativeRunId"] != run
                || cause["meaning"] != "cooperative-stop-cause-only"
                || cause["effectDisposition"] != "unknown"
                || !cause["cause"]
                    .as_str()
                    .is_some_and(|code| valid.contains(&code))
                || cause.as_object().is_none_or(|object| object.len() != 5)
            {
                bail!("native cancellation cause association invalid");
            }
            let digest = hex_sha256(&raw);
            observation["cooperativeStopCause"] = cause;
            observation["cooperativeStopCauseSha256"] = json!(digest);
            retained.push((format!("{digest}.cancellation-cause.json"), raw));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let bytes = serde_json::to_vec(&observation)?;
    let observation_hash = hex_sha256(&bytes);
    let report_identity = hex_sha256(&serde_json::to_vec(
        &json!({"device":device,"observationSha256":observation_hash}),
    )?);
    let report = json!({"reportId":format!("native-{report_identity}"),"deviceId":device,"runId":run,
        "attemptId":association["nativeAttemptId"],"publicationId":association["descriptor"]["publicationId"],
        "bundleSha256":association["descriptor"]["identity"]["sha256"],"outcome":"unknown","items":items});
    let evidence = dir.join("source-reports");
    for (name, raw) in retained {
        write_atomic(&evidence, &name, &raw)?;
    }
    write_atomic(
        &evidence,
        &format!("{snapshot_hash}.snapshot.json"),
        &snapshot,
    )?;
    write_atomic(&evidence, &format!("{observation_hash}.json"), &bytes)?;
    let package = json!({"source":association,"report":report,"observationSha256":observation_hash,
        "observation":observation,"qualification":"native-progress-only"});
    let encoded = serde_json::to_string(&package)?;
    write_atomic(
        &evidence,
        &format!("{report_identity}.report.json"),
        encoded.as_bytes(),
    )?;
    Ok(encoded)
}

#[cfg(test)]
mod snapshot_controls {
    use super::*;
    use std::collections::BTreeMap;
    use trollm_core::{artifacts::write_foreach_snapshot, EventLog, EventType};

    #[test]
    fn actual_writer_commit_cursor_and_report_sidecar_keep_original_zero_one_many() {
        let root = std::env::temp_dir().join(format!(
            "report-snapshot-{}",
            trollm_core::RunDir::under(Path::new("unused")).run_id()
        ));
        fs::create_dir_all(&root).unwrap();
        for values in [
            vec![],
            vec![json!({"id":"one","value":"é"})],
            vec![
                json!({"id":"one"}),
                json!({"id":"two"}),
                json!({"id":"three"}),
            ],
        ] {
            let written =
                write_foreach_snapshot(&root.join("artifacts"), "items", &values).unwrap();
            let reference = format!(
                "artifacts/{}",
                written.path.file_name().unwrap().to_str().unwrap()
            );
            let mut log =
                EventLog::open(&root.join("events.jsonl"), "declared-report-run").unwrap();
            log.append(
                EventType::ArtifactWritten,
                json!({"path":written.path,"sha256":written.sha256,"bytes":written.bytes}),
            )
            .unwrap();
            let mut committed = BTreeMap::new();
            EventLog::visit(
                &root.join("events.jsonl"),
                Some("declared-report-run"),
                |event, _| {
                    if event["type"] == "artifact_written" {
                        committed.insert(
                            event["path"].as_str().unwrap().to_owned(),
                            (
                                event["sha256"].as_str().unwrap().to_owned(),
                                event["bytes"].as_u64().unwrap(),
                            ),
                        );
                    }
                },
            )
            .unwrap();
            let (raw, read) =
                read_committed_cursor_snapshot(&root, &reference, &committed, &Default::default())
                    .unwrap();
            assert_eq!(read, values);
            let conflicts =
                std::collections::BTreeSet::from([written.path.to_string_lossy().into_owned()]);
            assert!(
                read_committed_cursor_snapshot(&root, &reference, &committed, &conflicts).is_err()
            );
            let sidecar = write_atomic(
                &root.join("source-reports"),
                &format!("{}.jsonl", hex_sha256(&raw)),
                &raw,
            )
            .unwrap();
            assert_eq!(
                fs::read(sidecar.path).unwrap(),
                fs::read(&written.path).unwrap()
            );
            for bad in [
                "artifacts/items.snapshot.jsonl",
                "../outside.snapshot.jsonl",
                "artifacts/missing.snapshot.jsonl",
            ] {
                assert!(read_committed_cursor_snapshot(
                    &root,
                    bad,
                    &committed,
                    &Default::default()
                )
                .is_err());
            }
            let mut wrong = committed.clone();
            wrong.get_mut(written.path.to_str().unwrap()).unwrap().1 += 1;
            assert!(
                read_committed_cursor_snapshot(&root, &reference, &wrong, &Default::default())
                    .is_err()
            );
            wrong = committed.clone();
            wrong.get_mut(written.path.to_str().unwrap()).unwrap().0 = "f".repeat(64);
            assert!(
                read_committed_cursor_snapshot(&root, &reference, &wrong, &Default::default())
                    .is_err()
            );
            assert!(read_committed_cursor_snapshot(
                &root,
                &reference,
                &BTreeMap::new(),
                &Default::default()
            )
            .is_err());
            fs::write(&written.path, b"dirty bytes").unwrap();
            assert!(read_committed_cursor_snapshot(
                &root,
                &reference,
                &committed,
                &Default::default()
            )
            .is_err());
            fs::write(&written.path, &raw).unwrap();
            #[cfg(unix)]
            {
                let alias = root.join("artifacts/alias.snapshot.jsonl");
                std::os::unix::fs::symlink(&written.path, &alias).unwrap();
                let mut aliases = committed.clone();
                aliases.insert(
                    alias.to_string_lossy().into_owned(),
                    (hex_sha256(&raw), raw.len() as u64),
                );
                assert!(read_committed_cursor_snapshot(
                    &root,
                    "artifacts/alias.snapshot.jsonl",
                    &aliases,
                    &Default::default()
                )
                .is_err());
                fs::remove_file(alias).unwrap();
            }
            drop(log);
        }
        fs::remove_dir_all(root).unwrap();
    }
}
