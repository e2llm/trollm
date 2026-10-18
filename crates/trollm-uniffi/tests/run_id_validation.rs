//! Run IDs are an FFI boundary even when the caller supplies a path to a real log.

use std::fs;
use trollm_uniffi::{TrollmError, TrollmRuntime};

const ID: &str = "abcdef01-2345-6789-abcd-ef0123456789";

#[test]
fn public_run_methods_reject_paths_and_noncanonical_ids() {
    let base = std::env::temp_dir().join(format!("trollm-run-id-{}", std::process::id()));
    let _ = fs::remove_dir_all(&base);
    let runtime = TrollmRuntime::new(base.to_string_lossy().into_owned(), None).unwrap();
    let outside = base.join("trollm/outside");
    fs::create_dir(&outside).unwrap();
    let event = br#"{"ts":1,"runId":"outside","seq":1,"type":"run_started"}
"#;
    fs::write(outside.join("events.jsonl"), event).unwrap();
    let invalid_ids = [
        "../outside".to_string(),
        outside.to_string_lossy().into_owned(),
        format!("../runs/{ID}"),
        ID.to_uppercase(),
        ID.replace('-', ""),
        "".into(),
    ];
    for id in invalid_ids {
        let errors = [
            runtime.run_summary(id.clone()).unwrap_err(),
            runtime.poll_status(id.clone()).unwrap_err(),
            runtime.tail_events(id.clone(), 0).unwrap_err(),
            runtime.resume(id.clone()).unwrap_err(),
            runtime.pause(id.clone()).unwrap_err(),
            runtime.cancel(id).unwrap_err(),
        ];
        for error in errors {
            assert!(matches!(error, TrollmError::Run(reason) if reason == "invalid run ID"));
        }
    }
    assert_eq!(fs::read(outside.join("events.jsonl")).unwrap(), event);
    assert_eq!(fs::read_dir(&outside).unwrap().count(), 1);
    assert_eq!(fs::read_dir(base.join("trollm/runs")).unwrap().count(), 0);
    fs::remove_dir_all(base).unwrap();
}

#[test]
fn canonical_missing_run_keeps_existing_not_found_and_summary_behavior() {
    let base = std::env::temp_dir().join(format!("trollm-valid-run-id-{}", std::process::id()));
    let _ = fs::remove_dir_all(&base);
    let runtime = TrollmRuntime::new(base.to_string_lossy().into_owned(), None).unwrap();
    let errors = [
        runtime.poll_status(ID.into()).unwrap_err(),
        runtime.tail_events(ID.into(), 0).unwrap_err(),
        runtime.resume(ID.into()).unwrap_err(),
        runtime.pause(ID.into()).unwrap_err(),
        runtime.cancel(ID.into()).unwrap_err(),
    ];
    for error in errors {
        assert!(matches!(error, TrollmError::NotFound(_)), "{error:?}");
    }
    let summary = runtime.run_summary(ID.into()).unwrap();
    assert_eq!(summary.run_id, ID);
    assert!(summary.phase.is_none());
    fs::remove_dir_all(base).unwrap();
}
