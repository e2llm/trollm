//! Lightweight projection of persisted run metadata for the history list.
//! Serde skips snapshot vars and outputs while reading; no event log is opened.

use std::fs::{self, File};
use std::io::BufReader;
use std::path::Path;

use serde::Deserialize;
use trollm_core::state::StateStatus;

use crate::run::{RunPhase, WORKER_FAILURE_FILE};
use crate::{RunPhaseApi, TrollmError};

#[derive(Debug, Clone, uniffi::Record)]
pub struct RunSummaryInfo {
    pub run_id: String,
    pub flow_id: Option<String>,
    pub flow_sha256: Option<String>,
    pub started_at_ms: Option<u64>,
    pub phase: Option<RunPhaseApi>,
    pub worker_active: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SnapshotProjection {
    #[serde(default)]
    status: Option<StateStatus>,
    #[serde(default)]
    stack: Vec<FrameProjection>,
    #[serde(default)]
    counters: CounterProjection,
    #[serde(default)]
    terminal_reason: Option<String>,
}

#[derive(Deserialize)]
struct FrameProjection {
    plan: String,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CounterProjection {
    #[serde(default)]
    started_at: u64,
}

#[derive(Deserialize)]
struct WorkerFailure {
    reason: String,
}

fn canonical_uuid(id: &str) -> bool {
    let bytes = id.as_bytes();
    bytes.len() == 36
        && bytes.iter().enumerate().all(|(index, byte)| {
            if matches!(index, 8 | 13 | 18 | 23) {
                *byte == b'-'
            } else {
                byte.is_ascii_digit() || (b'a'..=b'f').contains(byte)
            }
        })
}

fn pinned_flow(plan: &str) -> (Option<String>, Option<String>) {
    match plan.split_once("@sha256:") {
        Some((id, sha))
            if !id.is_empty() && sha.len() == 64 && sha.bytes().all(|b| b.is_ascii_hexdigit()) =>
        {
            (Some(id.to_string()), Some(sha.to_ascii_lowercase()))
        }
        _ => (None, None),
    }
}

/// Return one row even if its metadata is missing or corrupt. `live` is the
/// caller's coherent in-process status observation, when there is an active
/// handle; durable metadata remains the source for identity and start time.
pub fn read_run_summary(
    base: &Path,
    run_id: &str,
    live: Option<(RunPhase, bool)>,
) -> Result<RunSummaryInfo, TrollmError> {
    if !canonical_uuid(run_id) {
        return Err(TrollmError::Run("invalid run ID".into()));
    }
    let dir = base.join("runs").join(run_id);
    let snapshot_path = dir.join("state.snapshot.json");
    let snapshot = File::open(&snapshot_path).ok().and_then(|file| {
        serde_json::from_reader::<_, SnapshotProjection>(BufReader::new(file)).ok()
    });

    let (flow_id, flow_sha256) = snapshot
        .as_ref()
        .and_then(|saved| saved.stack.first())
        .map(|frame| pinned_flow(&frame.plan))
        .unwrap_or((None, None));
    let started_at_ms = snapshot
        .as_ref()
        .map(|saved| saved.counters.started_at)
        .filter(|time| *time > 0);

    let failure_path = dir.join(WORKER_FAILURE_FILE);
    let failure = fs::read(&failure_path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<WorkerFailure>(&bytes).ok());
    let disk_phase = if let Some(failure) = failure {
        Some(RunPhase::Failed {
            reason: failure.reason,
        })
    } else if failure_path.exists() {
        // A corrupt failure marker cannot be silently superseded by an older
        // snapshot; leave the row openable with unknown status.
        None
    } else {
        snapshot
            .as_ref()
            .and_then(|saved| match saved.status.as_ref()? {
                StateStatus::Running => Some(RunPhase::Running),
                StateStatus::Paused => Some(RunPhase::Paused {
                    reason: "paused (persisted)".into(),
                }),
                StateStatus::Succeeded => Some(RunPhase::Succeeded),
                StateStatus::Failed => Some(RunPhase::Failed {
                    reason: saved
                        .terminal_reason
                        .clone()
                        .unwrap_or_else(|| "failed (persisted)".into()),
                }),
            })
            .or_else(|| {
                if !snapshot_path.exists() && dir.join("events.jsonl").is_file() {
                    Some(RunPhase::Failed {
                        reason: "interrupted before saved progress; start a new run".into(),
                    })
                } else {
                    None
                }
            })
    };
    let (phase, worker_active) = match live {
        Some((phase, true)) => (Some(phase), true),
        Some((phase, false)) if matches!(&phase, RunPhase::Succeeded | RunPhase::Failed { .. }) => {
            (Some(phase), false)
        }
        _ => (disk_phase, false),
    };

    Ok(RunSummaryInfo {
        run_id: run_id.to_string(),
        flow_id,
        flow_sha256,
        started_at_ms,
        phase: phase.as_ref().map(RunPhaseApi::from),
        worker_active,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
    const SHA: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    fn with_run(test: impl FnOnce(&Path)) {
        let path = std::env::temp_dir().join(format!(
            "trollm-history-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let run = path.join("runs").join(ID);
        fs::create_dir_all(&run).unwrap();
        test(&path);
        fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn summary_projects_existing_snapshot_and_terminal_reason() {
        with_run(|base| {
            let run = base.join("runs").join(ID);
            fs::write(run.join("state.snapshot.json"), format!(
                "{{\"status\":\"failed\",\"terminalReason\":\"cancelled\",\"counters\":{{\"startedAt\":1700000000000}},\"stack\":[{{\"plan\":\"reader@sha256:{SHA}\",\"vars\":{{\"ignored\":[1,2]}}}}]}}"
            )).unwrap();
            let row = read_run_summary(base, ID, None).unwrap();
            assert_eq!(row.flow_id.as_deref(), Some("reader"));
            assert_eq!(row.flow_sha256.as_deref(), Some(SHA));
            assert_eq!(row.started_at_ms, Some(1_700_000_000_000));
            assert!(
                matches!(row.phase, Some(RunPhaseApi::Failed { reason }) if reason == "cancelled")
            );
        });
    }

    #[test]
    fn missing_or_corrupt_snapshot_keeps_unknown_row_and_failure_takes_precedence() {
        with_run(|base| {
            let run = base.join("runs").join(ID);
            let missing = read_run_summary(base, ID, None).unwrap();
            assert!(missing.phase.is_none());
            assert!(missing.started_at_ms.is_none());
            fs::write(run.join("state.snapshot.json"), b"{bad").unwrap();
            fs::write(
                run.join(WORKER_FAILURE_FILE),
                br#"{"reason":"worker stopped"}"#,
            )
            .unwrap();
            let failed = read_run_summary(base, ID, None).unwrap();
            assert!(failed.flow_id.is_none());
            assert!(
                matches!(failed.phase, Some(RunPhaseApi::Failed { reason }) if reason == "worker stopped")
            );
        });
    }

    #[test]
    fn event_file_without_snapshot_is_interrupted_before_saved_progress() {
        with_run(|base| {
            let run = base.join("runs").join(ID);
            fs::write(run.join("events.jsonl"), b"").unwrap();
            let row = read_run_summary(base, ID, None).unwrap();
            assert!(
                matches!(row.phase, Some(RunPhaseApi::Failed { reason }) if reason == "interrupted before saved progress; start a new run")
            );
            assert!(row.started_at_ms.is_none());
        });
    }

    #[test]
    fn invalid_id_cannot_reach_filesystem() {
        assert!(read_run_summary(Path::new("/tmp"), "../escape", None).is_err());
    }

    #[test]
    fn live_terminal_wins_over_older_snapshot_but_inactive_running_uses_disk() {
        with_run(|base| {
            let run = base.join("runs").join(ID);
            fs::write(run.join("state.snapshot.json"), br#"{"status":"paused"}"#).unwrap();
            let terminal = read_run_summary(base, ID, Some((RunPhase::Succeeded, false))).unwrap();
            assert!(matches!(terminal.phase, Some(RunPhaseApi::Succeeded)));
            let interrupted = read_run_summary(base, ID, Some((RunPhase::Running, false))).unwrap();
            assert!(matches!(
                interrupted.phase,
                Some(RunPhaseApi::Paused { .. })
            ));
            assert!(!interrupted.worker_active);
        });
    }

    #[test]
    fn corrupt_failure_marker_does_not_show_stale_success() {
        with_run(|base| {
            let run = base.join("runs").join(ID);
            fs::write(
                run.join("state.snapshot.json"),
                br#"{"status":"succeeded"}"#,
            )
            .unwrap();
            fs::write(run.join(WORKER_FAILURE_FILE), b"{bad").unwrap();
            let row = read_run_summary(base, ID, None).unwrap();
            assert!(row.phase.is_none());
        });
    }
}
