//! Integration tests for the UniFFI wrapper: bundle install/verify (default
//! features) and — behind `--features stub` — the full run lifecycle with the
//! deterministic stub model.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use trollm_uniffi::TrollmRuntime;

// ---------------------------------------------------------------------------
// fixtures

/// Minimal tar (ustar) writer for test bundles.
fn build_tar(files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut out = Vec::new();
    for (name, data) in files {
        let mut header = [0u8; 512];
        header[..name.len()].copy_from_slice(name.as_bytes());
        write_octal(&mut header[100..108], 0o644);
        write_octal(&mut header[108..116], 0);
        write_octal(&mut header[116..124], 0);
        write_octal(&mut header[124..136], data.len() as u64);
        write_octal(&mut header[136..148], 0);
        header[156] = b'0';
        header[257..262].copy_from_slice(b"ustar");
        for b in &mut header[148..156] {
            *b = b' ';
        }
        let sum: u64 = header.iter().map(|b| *b as u64).sum();
        write_octal(&mut header[148..156], sum);
        header[154] = 0;
        header[155] = b' ';
        out.extend_from_slice(&header);
        out.extend_from_slice(data);
        out.extend(std::iter::repeat_n(0u8, (512 - data.len() % 512) % 512));
    }
    out.extend(std::iter::repeat_n(0u8, 1024));
    out
}

fn write_octal(field: &mut [u8], value: u64) {
    let s = format!("{:0>width$}", format!("{value:o}"), width = field.len() - 1);
    field[..s.len()].copy_from_slice(s.as_bytes());
    field[field.len() - 1] = 0;
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::Digest;
    let mut h = sha2::Sha256::new();
    h.update(bytes);
    h.finalize().iter().fold(String::new(), |mut s, b| {
        write!(s, "{b:02x}").unwrap();
        s
    })
}

const GOOD_FLOW: &str = r#"
schemaVersion: ledger.execution-plan.v1
id: hello-flow
version: 1
sourceBaseline: { sha256: "0000000000000000000000000000000000000000000000000000000000000000" }
input:
  promptTemplate: "Greet {{ user_input }}"
limits: { maxSteps: 4, maxReasonerCalls: 2, maxTokensTotal: 10000, timeoutS: 60, artifactMaxMb: 10 }
onLimit: abort
models:
  coder: { ctx: 2048 }
mcp: { servers: [], policies: {} }
entryStep: greet
nodes:
  greet:
    kind: task
    taskRef: greet-task
    onVerified: done
  done: { kind: end, outcome: succeeded }
"#;

const GOOD_REGISTRY: &str = r#"
schemaVersion: ledger.task-registry.v1
id: hello
version: 1
tasks:
  greet-task:
    model: coder
    prompt: prompts/greet.md
    tools: []
    loop: { maxSteps: 1 }
    outputContract:
      outputs: { report: { from: model, type: prose } }
"#;

const GOOD_PROMPT: &str = "Hello {{ user_input }}";

/// A plan whose only task deterministically fails verification under the
/// stub model (tools declared but never called, allowNoop false) and whose
/// onError routes to a persisted pause — the plan-driven pause path.
#[cfg(all(feature = "stub", not(feature = "llama")))]
const PAUSING_FLOW: &str = r#"
schemaVersion: ledger.execution-plan.v1
id: pause-flow
version: 1
sourceBaseline: { sha256: "0000000000000000000000000000000000000000000000000000000000000000" }
input:
  promptTemplate: "Work on {{ user_input }}"
limits: { maxSteps: 4, maxReasonerCalls: 2, maxTokensTotal: 10000, timeoutS: 60, artifactMaxMb: 10 }
onLimit: abort
models:
  coder: { ctx: 2048 }
mcp: { servers: [], policies: {} }
entryStep: work
nodes:
  work:
    kind: task
    taskRef: failing-task
    onVerified: done
    onError: { retry: 0, then: { status: paused, reason: needs-attention } }
  done: { kind: end, outcome: succeeded }
"#;

#[cfg(all(feature = "stub", not(feature = "llama")))]
const PAUSING_REGISTRY: &str = r#"
schemaVersion: ledger.task-registry.v1
id: pauser
version: 1
tasks:
  failing-task:
    model: coder
    prompt: prompts/work.md
    tools: [srv.tool]
    loop: { maxSteps: 1 }
    outputContract:
      outputs: { outcome: { from: model, type: prose } }
"#;

/// flow id != file stem: rejected at install.
const WRONG_NAME_FLOW: &str = r#"
schemaVersion: ledger.execution-plan.v1
id: actual-id
version: 1
sourceBaseline: { sha256: "0000000000000000000000000000000000000000000000000000000000000000" }
limits: { maxSteps: 4, maxReasonerCalls: 2, maxTokensTotal: 10000, timeoutS: 60, artifactMaxMb: 10 }
onLimit: abort
models:
  coder: { ctx: 2048 }
entryStep: greet
nodes:
  greet:
    kind: task
    taskRef: greet-task
    onVerified: done
  done: { kind: end, outcome: succeeded }
"#;

fn manifest(entry_flow: &str, registry: &str, files: &[(&str, &[u8])]) -> Vec<u8> {
    let entries: Vec<String> = files
        .iter()
        .map(|(path, data)| {
            format!(
                "{{ \"path\": \"{path}\", \"sha256\": \"{}\" }}",
                sha256_hex(data)
            )
        })
        .collect();
    format!(
        "{{ \"format\": \"trollm.flow-bundle.v1\", \"entryFlow\": \"{entry_flow}\", \"taskRegistry\": \"{registry}\", \"files\": [{}] }}",
        entries.join(", ")
    )
    .into_bytes()
}

fn good_bundle() -> Vec<u8> {
    let files: Vec<(&str, &[u8])> = vec![
        ("hello-flow.flow.yaml", GOOD_FLOW.as_bytes()),
        ("hello.tasks.yaml", GOOD_REGISTRY.as_bytes()),
        ("prompts/greet.md", GOOD_PROMPT.as_bytes()),
    ];
    let manifest = manifest("hello-flow.flow.yaml", "hello.tasks.yaml", &files);
    let mut all: Vec<(&str, &[u8])> = vec![("manifest.json", &manifest)];
    all.extend(files);
    build_tar(&all)
}

#[cfg(all(feature = "stub", not(feature = "llama")))]
fn pausing_bundle() -> Vec<u8> {
    let files: Vec<(&str, &[u8])> = vec![
        ("pause-flow.flow.yaml", PAUSING_FLOW.as_bytes()),
        ("pauser.tasks.yaml", PAUSING_REGISTRY.as_bytes()),
        ("prompts/work.md", b"Work on {{ user_input }}"),
    ];
    let manifest = manifest("pause-flow.flow.yaml", "pauser.tasks.yaml", &files);
    let mut all: Vec<(&str, &[u8])> = vec![("manifest.json", &manifest)];
    all.extend(files);
    build_tar(&all)
}

fn temp_base(tag: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("trollm-uniffi-test-{}-{}", tag, std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

// ---------------------------------------------------------------------------
// bundle install tests (default features — no model runtime needed)

#[test]
fn install_verifies_and_lists() {
    let base = temp_base("install");
    let rt = TrollmRuntime::new(base.to_string_lossy().to_string(), None).unwrap();

    let result = rt.install_flow_bundle(good_bundle()).unwrap();
    assert_eq!(result.flow_id, "hello-flow");
    assert_eq!(result.flow_version, 1);
    assert_eq!(result.files_verified, 3);
    assert!(result.signature_status.contains("not-verified"));

    // §8 canonical pin + materialized bundle dir.
    let sha = &result.flow_sha256;
    assert!(base
        .join("trollm/flows")
        .join(format!("{sha}.flow.yaml"))
        .exists());
    assert!(base
        .join("trollm/bundles")
        .join(sha)
        .join("hello-flow.flow.yaml")
        .exists());
    assert!(base
        .join("trollm/bundles")
        .join(sha)
        .join("prompts/greet.md")
        .exists());
    let canonical =
        std::fs::read(base.join("trollm/flows").join(format!("{sha}.flow.yaml"))).unwrap();
    assert_eq!(sha256_hex(&canonical), *sha);

    let flows = rt.list_flows().unwrap();
    assert_eq!(flows.len(), 1);
    assert_eq!(flows[0].id, "hello-flow");
    assert_eq!(&flows[0].sha256, sha);

    // Idempotent reinstall of the identical bundle.
    rt.install_flow_bundle(good_bundle()).unwrap();
}

#[test]
fn install_bundle_without_source_baseline_preserves_raw_identity() {
    let base = temp_base("optional-provenance");
    let rt = TrollmRuntime::new(base.to_string_lossy().to_string(), None).unwrap();
    let flow = GOOD_FLOW.replace("sourceBaseline: { sha256: \"0000000000000000000000000000000000000000000000000000000000000000\" }\n", "");
    let files: Vec<(&str, &[u8])> = vec![
        ("hello-flow.flow.yaml", flow.as_bytes()),
        ("hello.tasks.yaml", GOOD_REGISTRY.as_bytes()),
        ("prompts/greet.md", GOOD_PROMPT.as_bytes()),
    ];
    let manifest = manifest("hello-flow.flow.yaml", "hello.tasks.yaml", &files);
    let mut all: Vec<(&str, &[u8])> = vec![("manifest.json", &manifest)];
    all.extend(files);
    let bundle = build_tar(&all);
    let installed = rt.install_flow_bundle(bundle.clone()).unwrap();
    let expected_sha = sha256_hex(flow.as_bytes());
    assert_eq!(installed.flow_sha256, expected_sha);
    assert_eq!(installed.files_verified, 3);
    let pin = base
        .join("trollm/flows")
        .join(format!("{expected_sha}.flow.yaml"));
    assert_eq!(std::fs::read(pin).unwrap(), flow.as_bytes());
    let flows = rt.list_flows().unwrap();
    assert_eq!(flows.len(), 1);
    assert_eq!(flows[0].sha256, expected_sha);
    assert_eq!(
        rt.install_flow_bundle(bundle).unwrap().flow_sha256,
        expected_sha
    );
}

#[test]
fn reinstall_repairs_missing_and_corrupt_discovery_pins() {
    let base = temp_base("repair-pins");
    let rt = TrollmRuntime::new(base.to_string_lossy().to_string(), None).unwrap();
    let installed = rt.install_flow_bundle(good_bundle()).unwrap();
    let flow_pin = base
        .join("trollm/flows")
        .join(format!("{}.flow.yaml", installed.flow_sha256));
    let task_pin = base.join("trollm/tasks").join(format!(
        "{}.tasks.yaml",
        sha256_hex(GOOD_REGISTRY.as_bytes())
    ));
    // Each state can remain after bundle publication but before pin publication.
    for missing_flow in [true, false] {
        if missing_flow {
            std::fs::remove_file(&flow_pin).unwrap();
        } else {
            std::fs::write(&flow_pin, b"partial flow pin").unwrap();
        }
        std::fs::write(&task_pin, b"partial task pin").unwrap();
        let restored = rt.install_flow_bundle(good_bundle()).unwrap();
        assert!(restored.signature_status.contains("not-verified"));
        assert_eq!(std::fs::read(&flow_pin).unwrap(), GOOD_FLOW.as_bytes());
        assert_eq!(std::fs::read(&task_pin).unwrap(), GOOD_REGISTRY.as_bytes());
        assert_eq!(rt.list_flows().unwrap().len(), 1);
    }
}

fn bundle_with_prompt(registry_path: &str, prompt: &str) -> Vec<u8> {
    let registry =
        GOOD_REGISTRY.replace("prompts/greet.md", &serde_json::to_string(prompt).unwrap());
    let files: Vec<(&str, &[u8])> = vec![
        ("hello-flow.flow.yaml", GOOD_FLOW.as_bytes()),
        (registry_path, registry.as_bytes()),
        ("prompts/greet.md", GOOD_PROMPT.as_bytes()),
    ];
    let manifest = manifest("hello-flow.flow.yaml", registry_path, &files);
    let mut all: Vec<(&str, &[u8])> = vec![("manifest.json", &manifest)];
    all.extend(files);
    build_tar(&all)
}

#[test]
fn prompts_must_resolve_to_verified_bundle_members() {
    let base = temp_base("prompt-boundary");
    let rt = TrollmRuntime::new(base.to_string_lossy().to_string(), None).unwrap();
    let external = base.join("trollm/bundles/external.md");
    std::fs::write(&external, b"outside the verified bundle").unwrap();
    for prompt in [
        "../external.md",
        external.to_str().unwrap(),
        "manifest.json",
    ] {
        let err = rt
            .install_flow_bundle(bundle_with_prompt("hello.tasks.yaml", prompt))
            .unwrap_err();
        assert!(err.to_string().contains("bundle"), "{err}");
        assert!(rt.list_flows().unwrap().is_empty());
    }
    // Relative traversal inside the verified root remains portable and valid.
    rt.install_flow_bundle(bundle_with_prompt(
        "registries/hello.tasks.yaml",
        "../prompts/greet.md",
    ))
    .unwrap();
    assert_eq!(rt.list_flows().unwrap().len(), 1);
}

#[test]
fn subflows_must_resolve_to_verified_bundle_members() {
    let base = temp_base("subflow-boundary");
    let rt = TrollmRuntime::new(base.to_string_lossy().to_string(), None).unwrap();
    std::fs::write(
        base.join("trollm/bundles/external.flow.yaml"),
        GOOD_FLOW.replace("id: hello-flow", "id: ../external"),
    )
    .unwrap();
    let flow = GOOD_FLOW.replace(
        "    taskRef: greet-task",
        "    taskRef: greet-task\n    body: { flowRef: ../external }",
    );
    let files: Vec<(&str, &[u8])> = vec![
        ("hello-flow.flow.yaml", flow.as_bytes()),
        ("hello.tasks.yaml", GOOD_REGISTRY.as_bytes()),
        ("prompts/greet.md", GOOD_PROMPT.as_bytes()),
    ];
    let manifest = manifest("hello-flow.flow.yaml", "hello.tasks.yaml", &files);
    let mut all: Vec<(&str, &[u8])> = vec![("manifest.json", &manifest)];
    all.extend(files);
    let err = rt.install_flow_bundle(build_tar(&all)).unwrap_err();
    assert!(
        err.to_string().contains("outside the verified manifest"),
        "{err}"
    );
    assert!(rt.list_flows().unwrap().is_empty());
}

#[cfg(any(feature = "stub", feature = "llama"))]
#[test]
fn startup_rechecks_installed_prompt_integrity() {
    let base = temp_base("startup-integrity");
    let rt = TrollmRuntime::new(base.to_string_lossy().to_string(), None).unwrap();
    let installed = rt.install_flow_bundle(good_bundle()).unwrap();
    std::fs::write(
        base.join("trollm/bundles")
            .join(&installed.flow_sha256)
            .join("prompts/greet.md"),
        b"modified",
    )
    .unwrap();
    let err = rt
        .start_run(installed.flow_sha256, "hello".into())
        .unwrap_err();
    assert!(err.to_string().contains("sha256 mismatch"), "{err}");
}

#[cfg(feature = "llama")]
#[test]
fn llama_selection_without_weights_fails_asynchronously_even_with_stub_enabled() {
    for manifest_present in [false, true] {
        let base = temp_base("llama-absent-weights");
        let rt = TrollmRuntime::new(base.to_string_lossy().to_string(), None).unwrap();
        let flow = rt.install_flow_bundle(good_bundle()).unwrap();
        if manifest_present {
            // Reach first-use verification, beyond legacy metadata capture. A
            // mistaken stub selection would succeed instead of rejecting the
            // absent native weights. No GGUF or native context is needed.
            let models = base.join("trollm/models");
            std::fs::create_dir_all(&models).unwrap();
            std::fs::write(models.join("coder.gguf.sha256"), "0".repeat(64)).unwrap();
        }
        let id = rt.start_run(flow.flow_sha256, "hello".into()).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            match rt.poll_status(id.clone()).unwrap().phase {
                trollm_uniffi::RunPhaseApi::Failed { reason } => {
                    if manifest_present {
                        assert_eq!(reason, "model-error");
                        let events = rt.tail_events(id.clone(), 0).unwrap();
                        assert!(
                            events.iter().any(|event| {
                                if event.event_type != "error" {
                                    return false;
                                }
                                let fields: serde_json::Value =
                                    serde_json::from_str(&event.json).unwrap();
                                fields["class"] == "model-error"
                                    && fields["node"] == "greet"
                                    && fields["detail"].as_str().is_some_and(|detail| {
                                        detail.contains("no weights for model slot 'coder'")
                                            && detail.contains("coder.gguf")
                                    })
                            }),
                            "missing first-use weights diagnostic: {events:?}"
                        );
                        assert!(
                            events.iter().all(|event| {
                                event.event_type != "model_call"
                                    && event.event_type != "tool_call_intent"
                            }),
                            "unexpected inference or tool dispatch: {events:?}"
                        );
                    } else {
                        assert!(
                            reason.contains("no SHA-256 manifest for model slot 'coder'"),
                            "{reason}"
                        );
                    }
                    break;
                }
                trollm_uniffi::RunPhaseApi::Running => {}
                other => panic!("unexpected phase: {other:?}"),
            }
            assert!(
                std::time::Instant::now() < deadline,
                "worker did not fail without weights"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }
}

#[test]
fn install_rejects_tampered_file() {
    let base = temp_base("tampered");
    let rt = TrollmRuntime::new(base.to_string_lossy().to_string(), None).unwrap();
    let mut bundle = good_bundle();
    // Flip a byte inside the flow file's data block. Layout: manifest header
    // (512) + manifest data blocks, then the flow header (512) + flow data.
    let manifest_len = manifest(
        "hello-flow.flow.yaml",
        "hello.tasks.yaml",
        &[
            ("hello-flow.flow.yaml", GOOD_FLOW.as_bytes()),
            ("hello.tasks.yaml", GOOD_REGISTRY.as_bytes()),
            ("prompts/greet.md", GOOD_PROMPT.as_bytes()),
        ],
    )
    .len();
    let flow_data_offset = 512 + 512 * manifest_len.div_ceil(512) + 512;
    bundle[flow_data_offset + 10] ^= 0xFF;
    let err = rt.install_flow_bundle(bundle).unwrap_err();
    assert!(
        err.to_string().contains("sha256 mismatch"),
        "unexpected error: {err}"
    );
    assert!(rt.list_flows().unwrap().is_empty());
    // Nothing staged left behind.
    assert!(std::fs::read_dir(base.join("trollm/bundles"))
        .unwrap()
        .next()
        .is_none());
}

#[test]
fn install_rejects_unlisted_member() {
    let base = temp_base("unlisted");
    let rt = TrollmRuntime::new(base.to_string_lossy().to_string(), None).unwrap();
    let files: Vec<(&str, &[u8])> = vec![
        ("hello-flow.flow.yaml", GOOD_FLOW.as_bytes()),
        ("hello.tasks.yaml", GOOD_REGISTRY.as_bytes()),
        ("prompts/greet.md", GOOD_PROMPT.as_bytes()),
    ];
    let manifest = manifest("hello-flow.flow.yaml", "hello.tasks.yaml", &files);
    let mut all: Vec<(&str, &[u8])> = vec![("manifest.json", &manifest)];
    all.extend(files);
    all.push(("evil.sh", b"rm -rf /")); // not in the manifest
    let err = rt.install_flow_bundle(build_tar(&all)).unwrap_err();
    assert!(err.to_string().contains("not listed"), "unexpected: {err}");
}

#[test]
fn install_rejects_invalid_plan() {
    let base = temp_base("invalid");
    let rt = TrollmRuntime::new(base.to_string_lossy().to_string(), None).unwrap();
    let bad_flow = GOOD_FLOW.replace("onVerified: done", "onVerified: nowhere");
    let files: Vec<(&str, &[u8])> = vec![
        ("hello-flow.flow.yaml", bad_flow.as_bytes()),
        ("hello.tasks.yaml", GOOD_REGISTRY.as_bytes()),
        ("prompts/greet.md", GOOD_PROMPT.as_bytes()),
    ];
    let manifest = manifest("hello-flow.flow.yaml", "hello.tasks.yaml", &files);
    let mut all: Vec<(&str, &[u8])> = vec![("manifest.json", &manifest)];
    all.extend(files);
    let err = rt.install_flow_bundle(build_tar(&all)).unwrap_err();
    assert!(err.to_string().contains("validation"), "unexpected: {err}");
    assert!(rt.list_flows().unwrap().is_empty());
}

#[test]
fn install_rejects_misnamed_flow_file() {
    let base = temp_base("misnamed");
    let rt = TrollmRuntime::new(base.to_string_lossy().to_string(), None).unwrap();
    let files: Vec<(&str, &[u8])> = vec![
        ("wrong-name.flow.yaml", WRONG_NAME_FLOW.as_bytes()),
        ("hello.tasks.yaml", GOOD_REGISTRY.as_bytes()),
        ("prompts/greet.md", GOOD_PROMPT.as_bytes()),
    ];
    let manifest = manifest("wrong-name.flow.yaml", "hello.tasks.yaml", &files);
    let mut all: Vec<(&str, &[u8])> = vec![("manifest.json", &manifest)];
    all.extend(files);
    let err = rt.install_flow_bundle(build_tar(&all)).unwrap_err();
    assert!(
        err.to_string().contains("must be named"),
        "unexpected: {err}"
    );
}

#[cfg(not(any(feature = "stub", feature = "llama")))]
#[test]
fn start_run_without_model_runtime_fails_cleanly() {
    let base = temp_base("nomodel");
    let rt = TrollmRuntime::new(base.to_string_lossy().to_string(), None).unwrap();
    let result = rt.install_flow_bundle(good_bundle()).unwrap();
    let err = rt
        .start_run(result.flow_sha256, "world".to_string())
        .unwrap_err();
    assert!(
        err.to_string().contains("no model runtime"),
        "unexpected: {err}"
    );
}

// ---------------------------------------------------------------------------
// run lifecycle tests (require the stub model)

#[cfg(all(feature = "stub", not(feature = "llama")))]
mod lifecycle {
    use super::*;
    use std::time::{Duration, Instant};
    use trollm_uniffi::RunPhaseApi;

    fn wait_for(
        rt: &TrollmRuntime,
        run_id: &str,
        pred: impl Fn(&RunPhaseApi) -> bool,
    ) -> RunPhaseApi {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            let phase = rt.poll_status(run_id.to_string()).unwrap().phase;
            if pred(&phase) {
                return phase;
            }
            assert!(
                Instant::now() < deadline,
                "timed out; last phase: {phase:?}"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn run_succeeds_and_reports() {
        let base = temp_base("succeeds");
        let rt = TrollmRuntime::new(base.to_string_lossy().to_string(), None).unwrap();
        let flow = rt.install_flow_bundle(good_bundle()).unwrap();
        let run_id = rt.start_run(flow.flow_sha256, "world".to_string()).unwrap();

        let phase = wait_for(&rt, &run_id, |p| !matches!(p, RunPhaseApi::Running));
        assert!(matches!(phase, RunPhaseApi::Succeeded), "got {phase:?}");

        let status = rt.poll_status(run_id.clone()).unwrap();
        assert_eq!(status.final_report.as_deref(), Some("stub model response"));
        assert!(status.last_seq > 0);

        // Tail semantics: full read, then incremental cursor.
        let events = rt.tail_events(run_id.clone(), 0).unwrap();
        let types: Vec<&str> = events.iter().map(|e| e.event_type.as_str()).collect();
        assert_eq!(types.first(), Some(&"run_started"));
        assert_eq!(types.last(), Some(&"run_ended"));
        assert!(types.contains(&"model_call"));
        let last = events.last().unwrap().seq;
        assert!(rt.tail_events(run_id.clone(), last).unwrap().is_empty());
        assert_eq!(rt.tail_events(run_id.clone(), last - 1).unwrap().len(), 1);
        // seq monotonic
        for w in events.windows(2) {
            assert!(w[0].seq < w[1].seq);
        }
    }

    #[test]
    fn plan_pause_resume_then_cancel() {
        let base = temp_base("pause");
        let rt = TrollmRuntime::new(base.to_string_lossy().to_string(), None).unwrap();
        let flow = rt.install_flow_bundle(pausing_bundle()).unwrap();
        let run_id = rt.start_run(flow.flow_sha256, "x".to_string()).unwrap();

        let phase = wait_for(&rt, &run_id, |p| !matches!(p, RunPhaseApi::Running));
        match &phase {
            RunPhaseApi::Paused { reason } => assert_eq!(reason, "needs-attention"),
            other => panic!("expected paused, got {other:?}"),
        }

        // Resume re-runs the failing task and pauses again (deterministic).
        rt.resume(run_id.clone()).unwrap();
        let phase = wait_for(&rt, &run_id, |p| !matches!(p, RunPhaseApi::Running));
        assert!(matches!(phase, RunPhaseApi::Paused { .. }), "got {phase:?}");

        // Cancel a paused run goes through Executor::cancel: the log records
        // run_ended{outcome:"cancelled"}.
        rt.cancel(run_id.clone()).unwrap();
        let phase = wait_for(&rt, &run_id, |p| matches!(p, RunPhaseApi::Failed { .. }));
        match &phase {
            RunPhaseApi::Failed { reason } => assert_eq!(reason, "cancelled"),
            other => panic!("expected failed/cancelled, got {other:?}"),
        }
        let events = rt.tail_events(run_id.clone(), 0).unwrap();
        let ended = events.iter().find(|e| e.event_type == "run_ended").unwrap();
        assert!(ended.json.contains("\"cancelled\""), "{}", ended.json);
    }

    #[test]
    fn cold_resume_after_process_death() {
        let base = temp_base("coldresume");
        let run_id;
        let flow_sha;
        {
            let rt = TrollmRuntime::new(base.to_string_lossy().to_string(), None).unwrap();
            let flow = rt.install_flow_bundle(pausing_bundle()).unwrap();
            flow_sha = flow.flow_sha256;
            run_id = rt.start_run(flow_sha.clone(), "x".to_string()).unwrap();
            wait_for(&rt, &run_id, |p| matches!(p, RunPhaseApi::Paused { .. }));
            // Snapshot persisted; simulate process death by dropping the runtime.
        }

        // Fresh runtime over the same files dir: poll_status reads the
        // snapshot from disk; resume() cold-restores the executor.
        let rt = TrollmRuntime::new(base.to_string_lossy().to_string(), None).unwrap();
        let status = rt.poll_status(run_id.clone()).unwrap();
        assert!(matches!(status.phase, RunPhaseApi::Paused { .. }));

        rt.resume(run_id.clone()).unwrap();
        let phase = wait_for(&rt, &run_id, |p| {
            matches!(p, RunPhaseApi::Paused { .. })
                && rt
                    .tail_events(run_id.clone(), 0)
                    .unwrap()
                    .iter()
                    .any(|e| e.event_type == "resumed")
        });
        let _ = phase;
        let events = rt.tail_events(run_id, 0).unwrap();
        assert!(events.iter().any(|e| e.event_type == "resumed"));
        let _ = flow_sha;
    }

    #[test]
    fn structured_input_is_canonical_persisted_and_revalidated_on_cold_resume() {
        let base = temp_base("structured-input-resume");
        let rt = TrollmRuntime::new(base.to_string_lossy().to_string(), None).unwrap();
        let mut descriptor = setup_fixture();
        descriptor["input"] = serde_json::json!({"fields":[{"key":"browser","label":"Browser","required":true}] ,"toolArguments":{"srv.tool":{"fromInput":{"browser":"browser"}}}});
        let bytes = serde_json::to_vec(&descriptor).unwrap();
        let flow = format!(
            "{}\nsetup: {{path: setup.json, sha256: '{}'}}\n",
            PAUSING_FLOW.replace(
                "servers: []",
                "servers: [{name: srv, url: 'https://example.invalid/mcp'}]"
            ),
            sha256_hex(&bytes)
        );
        let files: Vec<(&str, &[u8])> = vec![
            ("pause-flow.flow.yaml", flow.as_bytes()),
            ("pauser.tasks.yaml", PAUSING_REGISTRY.as_bytes()),
            ("prompts/work.md", b"Work on {{ user_input }}"),
            ("setup.json", &bytes),
        ];
        let manifest = manifest("pause-flow.flow.yaml", "pauser.tasks.yaml", &files);
        let mut all = vec![("manifest.json", manifest.as_slice())];
        all.extend(files);
        let flow = rt.install_flow_bundle(build_tar(&all)).unwrap();
        assert!(rt
            .start_run(flow.flow_sha256.clone(), "not JSON".into())
            .is_err());
        let id = rt
            .start_run(flow.flow_sha256, r#"{"browser":" chosen "}"#.into())
            .unwrap();
        wait_for(&rt, &id, |p| matches!(p, RunPhaseApi::Paused { .. }));
        let input_path = base.join("trollm/runs").join(&id).join("user_input.txt");
        assert_eq!(
            std::fs::read_to_string(&input_path).unwrap(),
            r#"{"browser":"chosen"}"#
        );
        drop(rt);
        let rt = TrollmRuntime::new(base.to_string_lossy().to_string(), None).unwrap();
        rt.resume(id.clone()).unwrap();
        wait_for(&rt, &id, |p| {
            matches!(p, RunPhaseApi::Paused { .. })
                && rt
                    .tail_events(id.clone(), 0)
                    .unwrap()
                    .iter()
                    .any(|e| e.event_type == "resumed")
        });
        drop(rt);
        std::fs::write(&input_path, r#"{"browser":"chosen","unexpected":"x"}"#).unwrap();
        let rt = TrollmRuntime::new(base.to_string_lossy().to_string(), None).unwrap();
        assert!(rt.resume(id).is_err());
    }

    #[test]
    fn configured_run_preserves_effective_settings_on_cold_resume() {
        let base = temp_base("configured-resume");
        let rt = TrollmRuntime::new(base.to_string_lossy().to_string(), None).unwrap();
        let flow = rt.install_flow_bundle(pausing_bundle()).unwrap();
        let configuration = serde_json::json!({"version":1,"models":{"coder":{
            "path":base.join("custom.gguf").to_string_lossy(),
            "sha256":"a".repeat(64),"ctx":4096,"temperature":0.4,"topP":0.8
        }},"connections":{}});
        let run_id = rt
            .start_run_configured(
                flow.flow_sha256.clone(),
                "x".into(),
                configuration.to_string(),
            )
            .unwrap();
        wait_for(&rt, &run_id, |p| matches!(p, RunPhaseApi::Paused { .. }));
        let record = base
            .join("trollm/runs")
            .join(&run_id)
            .join("configuration.json");
        let before: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&record).unwrap()).unwrap();
        assert_eq!(before, configuration);
        drop(rt);
        // A previous cold-start failure must stop overriding the snapshot once
        // its cause is repaired and executor reconstruction succeeds.
        let old_failure = record.with_file_name("worker-failure.json");
        std::fs::write(
            &old_failure,
            br#"{"reason":"temporary model load failure"}"#,
        )
        .unwrap();
        let rt = TrollmRuntime::new(base.to_string_lossy().to_string(), None).unwrap();
        rt.resume(run_id.clone()).unwrap();
        wait_for(&rt, &run_id, |p| {
            matches!(p, RunPhaseApi::Paused { .. })
                && rt
                    .tail_events(run_id.clone(), 0)
                    .unwrap()
                    .iter()
                    .any(|e| e.event_type == "resumed")
        });
        let after: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&record).unwrap()).unwrap();
        assert_eq!(after, before);
        assert!(!old_failure.exists());
        assert!(matches!(
            trollm_uniffi::run::status_from_disk(&base.join("trollm"), &run_id).unwrap(),
            Some(trollm_uniffi::run::RunPhase::Paused { .. })
        ));
        // The old start API and setup inspection still support this legacy bundle.
        assert!(rt.flow_setup(flow.flow_sha256.clone()).unwrap().is_none());
        let mut invalid = configuration;
        invalid["models"]["undeclared"] = invalid["models"]["coder"].clone();
        assert!(rt
            .start_run_configured(flow.flow_sha256, "x".into(), invalid.to_string())
            .is_err());
        rt.cancel(run_id.clone()).unwrap();
        wait_for(&rt, &run_id, |p| matches!(p, RunPhaseApi::Failed { .. }));
    }

    #[test]
    fn startup_event_polling_recognizes_run_before_executor_initialization() {
        use std::sync::{mpsc, Mutex};
        use trollm_uniffi::{ConnectionAuth, TokenProvider, TrollmError};

        struct StartupProvider {
            entered: mpsc::Sender<()>,
            release: Mutex<mpsc::Receiver<()>>,
        }
        impl TokenProvider for StartupProvider {
            fn bearer_token(&self, _: String, _: String) -> Option<String> {
                None
            }

            fn connection_auth(&self, _: String, _: String) -> ConnectionAuth {
                let _ = self.entered.send(());
                // Bound the wait even if the test unwinds before releasing us.
                let _ = self
                    .release
                    .lock()
                    .unwrap()
                    .recv_timeout(Duration::from_secs(10));
                ConnectionAuth::Unavailable
            }
        }
        struct ReleaseOnDrop(mpsc::Sender<()>);
        impl Drop for ReleaseOnDrop {
            fn drop(&mut self) {
                let _ = self.0.send(());
            }
        }

        let base = temp_base("startup-event-polling");
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let release = ReleaseOnDrop(release_tx);
        let rt = TrollmRuntime::new(
            base.to_string_lossy().to_string(),
            Some(Box::new(StartupProvider {
                entered: entered_tx,
                release: Mutex::new(release_rx),
            })),
        )
        .unwrap();
        let flow_text = GOOD_FLOW.replace(
            "mcp: { servers: [], policies: {} }",
            "mcp: { servers: [{ name: browser, url: 'https://example.org/mcp' }], policies: {} }",
        );
        let files = vec![
            ("hello-flow.flow.yaml", flow_text.as_bytes()),
            ("hello.tasks.yaml", GOOD_REGISTRY.as_bytes()),
            ("prompts/greet.md", GOOD_PROMPT.as_bytes()),
        ];
        let manifest = manifest("hello-flow.flow.yaml", "hello.tasks.yaml", &files);
        let mut entries = files;
        entries.push(("manifest.json", &manifest));
        let flow = rt.install_flow_bundle(build_tar(&entries)).unwrap();
        let id = rt.start_run(flow.flow_sha256, "x".into()).unwrap();

        // The callback blocks worker startup before model construction and
        // Executor::new, so this is not a timing race with a fast stub run.
        entered_rx.recv_timeout(Duration::from_secs(10)).unwrap();
        let run_dir = base.join("trollm/runs").join(&id);
        assert!(!run_dir.join("state.snapshot.json").exists());
        assert_eq!(
            std::fs::metadata(run_dir.join("events.jsonl"))
                .unwrap()
                .len(),
            0
        );
        assert!(rt.tail_events(id.clone(), 0).unwrap().is_empty());
        assert!(matches!(
            rt.poll_status(id.clone()).unwrap().phase,
            RunPhaseApi::Running
        ));
        let unknown = "00000000-0000-0000-0000-000000000000".to_string();
        assert!(matches!(
            rt.tail_events(unknown.clone(), 0),
            Err(TrollmError::NotFound(_))
        ));
        assert!(matches!(
            rt.poll_status(unknown),
            Err(TrollmError::NotFound(_))
        ));

        // Unavailable authentication fails without network or model loading;
        // the early empty log must not hide the durable startup error.
        drop(release);
        let expected = match wait_for(&rt, &id, |phase| {
            matches!(phase, RunPhaseApi::Failed { .. })
        }) {
            RunPhaseApi::Failed { reason } => reason,
            _ => unreachable!(),
        };
        assert!(expected.contains("needs sign-in"), "{expected}");
        assert!(!run_dir.join("state.snapshot.json").exists());
        drop(rt);
        let reopened = TrollmRuntime::new(base.to_string_lossy().to_string(), None).unwrap();
        assert!(matches!(reopened.poll_status(id.clone()).unwrap().phase,
            RunPhaseApi::Failed { reason } if reason == expected));
        assert!(reopened.tail_events(id, 0).unwrap().is_empty());
        drop(reopened);
        std::fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn startup_failure_remains_visible_after_runtime_restart() {
        let base = temp_base("durable-startup-failure");
        let rt = TrollmRuntime::new(base.to_string_lossy().to_string(), None).unwrap();
        let flow_text = GOOD_FLOW.replace(
            "mcp: { servers: [], policies: {} }",
            "mcp: { servers: [{ name: browser, url: 'https://example.org/mcp' }], policies: {} }",
        );
        let files: Vec<(&str, &[u8])> = vec![
            ("hello-flow.flow.yaml", flow_text.as_bytes()),
            ("hello.tasks.yaml", GOOD_REGISTRY.as_bytes()),
            ("prompts/greet.md", GOOD_PROMPT.as_bytes()),
        ];
        let manifest = manifest("hello-flow.flow.yaml", "hello.tasks.yaml", &files);
        let mut entries = files;
        entries.push(("manifest.json", &manifest));
        let flow = rt.install_flow_bundle(build_tar(&entries)).unwrap();
        let trust = base.join("trollm/trust");
        std::fs::create_dir_all(&trust).unwrap();
        std::fs::write(trust.join("mcp-roots.pem"), b"invalid certificate").unwrap();
        let id = rt.start_run(flow.flow_sha256, "x".into()).unwrap();
        let phase = wait_for(&rt, &id, |p| matches!(p, RunPhaseApi::Failed { .. }));
        let expected = match phase {
            RunPhaseApi::Failed { reason } => reason,
            _ => unreachable!(),
        };
        assert!(expected.contains("root certificates"), "{expected}");
        assert!(!base
            .join("trollm/runs")
            .join(&id)
            .join("state.snapshot.json")
            .exists());
        drop(rt);
        let reopened = TrollmRuntime::new(base.to_string_lossy().to_string(), None).unwrap();
        assert!(matches!(reopened.poll_status(id.clone()).unwrap().phase,
            RunPhaseApi::Failed { reason } if reason == expected));
        assert!(reopened.tail_events(id, 0).unwrap().is_empty());
    }
}

/// Keep Path import used when stub is off.
#[allow(unused)]
fn _unused(p: &Path) {}

// Optional setup defaults stay inside the existing manifest/hash boundary.
fn setup_fixture() -> serde_json::Value {
    serde_json::json!({
        "format":"trollm.flow-setup.v1", "title":"Greeting", "input":{"label":"Name"},
        "models":{"coder":{"label":"Coder","url":"https://example.org/model.gguf",
            "sha256":"a".repeat(64),"sizeBytes":123,"ctx":2048}},
        "connections":{}
    })
}

fn setup_bundle(descriptor: Option<&[u8]>, pin: &str) -> Vec<u8> {
    let flow = format!("{GOOD_FLOW}\nsetup: {{ path: setup.json, sha256: '{pin}' }}\n");
    let mut files: Vec<(&str, &[u8])> = vec![
        ("hello-flow.flow.yaml", flow.as_bytes()),
        ("hello.tasks.yaml", GOOD_REGISTRY.as_bytes()),
        ("prompts/greet.md", GOOD_PROMPT.as_bytes()),
    ];
    if let Some(bytes) = descriptor {
        files.push(("setup.json", bytes));
    }
    let manifest = manifest("hello-flow.flow.yaml", "hello.tasks.yaml", &files);
    let mut all = vec![("manifest.json", manifest.as_slice())];
    all.extend(files);
    build_tar(&all)
}

#[test]
fn setup_install_pins_descriptor_and_changes_root_identity() {
    let base = temp_base("setup-pins");
    let first = serde_json::to_vec(&setup_fixture()).unwrap();
    let a = trollm_uniffi::bundle::install_bundle(
        &base,
        &setup_bundle(Some(&first), &sha256_hex(&first)),
    )
    .unwrap();
    let mut changed = setup_fixture();
    changed["title"] = "Different greeting".into();
    let second = serde_json::to_vec(&changed).unwrap();
    let b = trollm_uniffi::bundle::install_bundle(
        &base,
        &setup_bundle(Some(&second), &sha256_hex(&second)),
    )
    .unwrap();
    assert_ne!(a.flow_sha256, b.flow_sha256);
    assert!(a.bundle_dir.join("setup.json").is_file());
    assert!(trollm_uniffi::bundle::install_bundle(
        &base,
        &setup_bundle(Some(&second), &sha256_hex(&first))
    )
    .is_err());
    std::fs::remove_dir_all(base).unwrap();
}

#[test]
fn setup_install_rejects_missing_invalid_version_and_unknown_fields() {
    let base = temp_base("setup-invalid");
    let bytes = serde_json::to_vec(&setup_fixture()).unwrap();
    assert!(
        trollm_uniffi::bundle::install_bundle(&base, &setup_bundle(None, &sha256_hex(&bytes)))
            .is_err()
    );
    for field in ["format", "unknown"] {
        let mut value = setup_fixture();
        value[field] = "unsupported".into();
        let bytes = serde_json::to_vec(&value).unwrap();
        assert!(trollm_uniffi::bundle::install_bundle(
            &base,
            &setup_bundle(Some(&bytes), &sha256_hex(&bytes))
        )
        .is_err());
    }
    if base.exists() {
        std::fs::remove_dir_all(base).unwrap();
    }
}

#[test]
fn setup_startup_rejects_descriptor_tamper_even_if_manifest_hash_is_updated() {
    let base = temp_base("setup-startup");
    let rt = TrollmRuntime::new(base.to_string_lossy().to_string(), None).unwrap();
    let bytes = serde_json::to_vec(&setup_fixture()).unwrap();
    let installed = rt
        .install_flow_bundle(setup_bundle(Some(&bytes), &sha256_hex(&bytes)))
        .unwrap();
    let dir = base.join("trollm/bundles").join(&installed.flow_sha256);
    let changed = b"{}";
    std::fs::write(dir.join("setup.json"), changed).unwrap();
    let path = dir.join("manifest.json");
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    for file in manifest["files"].as_array_mut().unwrap() {
        if file["path"] == "setup.json" {
            file["sha256"] = sha256_hex(changed).into();
        }
    }
    std::fs::write(path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    let error = rt.flow_setup(installed.flow_sha256.clone()).unwrap_err();
    assert!(error.to_string().contains("root flow pin"), "{error}");
    // No-runtime builds intentionally fail before loading a run plan. Setup
    // inspection above must still enforce the pin in every feature variant.
    #[cfg(any(feature = "stub", feature = "llama"))]
    {
        let error = rt
            .start_run(installed.flow_sha256, "hello".into())
            .unwrap_err();
        assert!(error.to_string().contains("root flow pin"), "{error}");
    }
    std::fs::remove_dir_all(base).unwrap();
}

#[test]
fn new_import_rejects_short_provenance_but_installed_legacy_bundle_remains_readable() {
    let base = temp_base("legacy-provenance");
    let hash = "0".repeat(64);
    let legacy_flow = GOOD_FLOW.replace(&hash, "deadbeef");
    let legacy_registry = format!("{GOOD_REGISTRY}\nsourceBaseline: {{ sha256: deadbeef }}\n");
    let files: Vec<(&str, &[u8])> = vec![
        ("hello-flow.flow.yaml", legacy_flow.as_bytes()),
        ("hello.tasks.yaml", legacy_registry.as_bytes()),
        ("prompts/greet.md", GOOD_PROMPT.as_bytes()),
    ];
    let manifest_bytes = manifest("hello-flow.flow.yaml", "hello.tasks.yaml", &files);
    let mut all: Vec<(&str, &[u8])> = vec![("manifest.json", &manifest_bytes)];
    all.extend(files.iter().copied());
    let archive = build_tar(&all);
    let rt = TrollmRuntime::new(base.to_string_lossy().to_string(), None).unwrap();
    let error = rt.install_flow_bundle(archive.clone()).unwrap_err();
    assert!(
        error.to_string().contains("sourceBaseline.sha256"),
        "{error}"
    );
    assert!(rt.list_flows().unwrap().is_empty());
    drop(rt);

    // Reproduce a materialized pre-upgrade install without passing it through
    // the new-import boundary. Its raw bytes and manifest remain unchanged.
    let sha = sha256_hex(legacy_flow.as_bytes());
    let installed = base.join("trollm/bundles").join(&sha);
    for (path, bytes) in &all {
        let path = installed.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }
    let flow_pin = base.join("trollm/flows").join(format!("{sha}.flow.yaml"));
    std::fs::create_dir_all(flow_pin.parent().unwrap()).unwrap();
    std::fs::write(&flow_pin, legacy_flow.as_bytes()).unwrap();
    let task_sha = sha256_hex(legacy_registry.as_bytes());
    let task_pin = base
        .join("trollm/tasks")
        .join(format!("{task_sha}.tasks.yaml"));
    std::fs::create_dir_all(task_pin.parent().unwrap()).unwrap();
    std::fs::write(&task_pin, legacy_registry.as_bytes()).unwrap();

    let rt = TrollmRuntime::new(base.to_string_lossy().to_string(), None).unwrap();
    assert_eq!(rt.list_flows().unwrap()[0].sha256, sha);
    // This exercises the shared verified loader used by installed run startup.
    assert!(rt.flow_setup(sha.clone()).unwrap().is_none());
    std::fs::remove_file(&flow_pin).unwrap();
    std::fs::remove_file(&task_pin).unwrap();
    let restored = rt.install_flow_bundle(archive).unwrap();
    assert_eq!(restored.flow_sha256, sha);
    assert_eq!(std::fs::read(&flow_pin).unwrap(), legacy_flow.as_bytes());
    assert_eq!(
        std::fs::read(&task_pin).unwrap(),
        legacy_registry.as_bytes()
    );
    assert_eq!(
        std::fs::read(installed.join("manifest.json")).unwrap(),
        manifest_bytes
    );
    drop(rt);
    std::fs::remove_dir_all(base).unwrap();
}

#[test]
fn new_bundle_import_rejects_authoring_typos_and_reasoner_tool_grants() {
    let base = temp_base("closed-authoring");
    let rt = TrollmRuntime::new(base.to_string_lossy().to_string(), None).unwrap();
    for (flow, registry, expected) in [
        (
            GOOD_FLOW.replace(
                "taskRef: greet-task",
                "taskRef: greet-task\n    onEror: { retry: 2 }",
            ),
            GOOD_REGISTRY.to_string(),
            "onEror",
        ),
        (
            GOOD_FLOW.to_string(),
            GOOD_REGISTRY.replace(
                "tools: []",
                "tools: []\n    mutation: { idempotency: intent-log }",
            ),
            "mutation",
        ),
        (
            GOOD_FLOW.to_string(),
            GOOD_REGISTRY
                .replace("model: coder", "model: reasoner")
                .replace("tools: []", "tools: [srv.read]"),
            "reasoner",
        ),
        (
            GOOD_FLOW.to_string(),
            format!("{GOOD_REGISTRY}\nsourceBaseline: {{ sha256: deadbeef }}\n"),
            "sourceBaseline.sha256",
        ),
        (
            GOOD_FLOW.replace(
                "policies: {}",
                "policies: { srv.read: { rety: { attempts: 2 } } }",
            ),
            GOOD_REGISTRY.to_string(),
            "rety",
        ),
    ] {
        let files: Vec<(&str, &[u8])> = vec![
            ("hello-flow.flow.yaml", flow.as_bytes()),
            ("hello.tasks.yaml", registry.as_bytes()),
            ("prompts/greet.md", GOOD_PROMPT.as_bytes()),
        ];
        let manifest_bytes = manifest("hello-flow.flow.yaml", "hello.tasks.yaml", &files);
        let mut all: Vec<(&str, &[u8])> = vec![("manifest.json", &manifest_bytes)];
        all.extend(files);
        let error = rt.install_flow_bundle(build_tar(&all)).unwrap_err();
        assert!(
            error.to_string().contains(expected),
            "expected {expected}: {error}"
        );
        assert!(rt.list_flows().unwrap().is_empty());
    }
    drop(rt);
    std::fs::remove_dir_all(base).unwrap();
}
