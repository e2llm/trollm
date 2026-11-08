//! Closed selected-start diagnostics. Neither a worker binding nor a stop ACK.
use anyhow::Result;
use std::path::Path;

#[derive(Debug)]
pub(crate) struct Failure {
    pub code: &'static str,
    pub native_run_id: Option<String>,
}
impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.code)
    }
}
impl std::error::Error for Failure {}

pub(crate) fn at<T>(result: Result<T>, code: &'static str, directory: &Path) -> Result<T> {
    result.map_err(|error| {
        let code = closed_code(&error, code);
        error.context(Failure {
            code,
            native_run_id: directory
                .file_name()
                .and_then(|name| name.to_str())
                .map(str::to_owned),
        })
    })
}

pub(crate) fn closed_code(error: &anyhow::Error, fallback: &'static str) -> &'static str {
    error
        .downcast_ref::<Failure>()
        .map(|cause| cause.code)
        .or_else(|| {
            error
                .downcast_ref::<crate::collection_clock::ClockFailure>()
                .map(|cause| cause.0)
        })
        .unwrap_or(fallback)
}

pub(crate) fn boundary<T>(result: Result<T>, code: &'static str) -> Result<T> {
    result.map_err(|error| {
        let code = closed_code(&error, code);
        error.context(Failure {
            code,
            native_run_id: None,
        })
    })
}

pub(crate) fn retain_failure(directory: &Path, code: &'static str) -> Result<()> {
    let record = serde_json::json!({"schema":"trollm.selected-start-failure.v1-proposal",
        "nativeRunId":directory.file_name().and_then(|name|name.to_str()),"code":code,
        "meaning":"start-refusal-diagnostic-only","effectDisposition":"unknown",
        "workerInactiveAcknowledged":false});
    trollm_core::artifacts::write_atomic(
        directory,
        "selected-start-failure.json",
        &serde_json::to_vec(&record)?,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn typed_refusal_preserves_real_allocated_id_but_never_raw_error_or_stop_ack() {
        let root = std::env::temp_dir().join(format!("selected-failure-{}", std::process::id()));
        let directory = root.join("00000000-0000-4000-8000-000000000001");
        std::fs::create_dir_all(&directory).unwrap();
        let error = at::<()>(
            Err(anyhow::anyhow!("private-source-and-key-must-not-be-public")),
            "SELECTED_BIND_STATIC_KEY_HASH",
            &directory,
        )
        .unwrap_err();
        let public = crate::TrollmError::selected_start(error);
        assert_eq!(
            public.to_string(),
            "selected start refused: SELECTED_BIND_STATIC_KEY_HASH"
        );
        match public {
            crate::TrollmError::SelectedStart {
                code,
                native_run_id,
            } => {
                assert_eq!(code, "SELECTED_BIND_STATIC_KEY_HASH");
                assert_eq!(
                    native_run_id.as_deref(),
                    Some("00000000-0000-4000-8000-000000000001")
                );
            }
            _ => panic!("typed selected refusal required"),
        }
        retain_failure(&directory, "SELECTED_BIND_STATIC_KEY_HASH").unwrap();
        let record: serde_json::Value = serde_json::from_slice(
            &std::fs::read(directory.join("selected-start-failure.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(record["workerInactiveAcknowledged"], false);
        assert_eq!(record["effectDisposition"], "unknown");
        assert!(!directory.join("events.jsonl").exists());
        assert!(!directory.join("completion.json").exists());
        std::fs::remove_dir_all(root).unwrap();
    }
}
