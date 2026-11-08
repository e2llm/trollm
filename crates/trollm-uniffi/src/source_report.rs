//! Native progress evidence, never model-text or website verification.
use crate::{bundle, RunPhaseApi, RunStatusInfo};
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::{
    fs,
    path::{Component, Path},
};
use trollm_core::{artifacts::write_atomic, plan::hex_sha256, RunState};

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
    let root = dir.canonicalize()?;
    let mut committed_artifacts = std::collections::BTreeMap::new();
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
            committed_artifacts.insert(path.to_string(), (sha.to_string(), bytes));
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
        let relative = Path::new(&cursor.snapshot_ref);
        if relative
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
        {
            bail!("invalid native snapshot reference");
        }
        let logical_path = dir.join(relative);
        let path = logical_path.canonicalize()?;
        if !path.starts_with(&root) {
            bail!("native snapshot escapes run custody");
        }
        if fs::metadata(&path)?.len() > 32 * 1024 * 1024 {
            bail!("native snapshot report limit");
        }
        let raw = fs::read(&path)?;
        if raw.len() > 32 * 1024 * 1024 {
            bail!("native snapshot report limit");
        }
        let values: Vec<Value> = std::str::from_utf8(&raw)?
            .lines()
            .filter(|line| !line.is_empty())
            .map(serde_json::from_str)
            .collect::<std::result::Result<_, _>>()?;
        if items.len() + values.len() > 128
            || cursor.results.len() > values.len()
            || cursor.index > values.len()
        {
            bail!("native item report limit or inconsistent prefix");
        }
        let raw_hash = hex_sha256(&raw);
        if !committed_artifacts
            .get(logical_path.to_string_lossy().as_ref())
            .is_some_and(|(hash, bytes)| hash == &raw_hash && *bytes == raw.len() as u64)
        {
            bail!("native item snapshot differs from its committed artifact identity");
        }
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
