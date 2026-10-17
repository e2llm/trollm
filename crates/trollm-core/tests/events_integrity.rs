//! Journal integrity checks use scripted models/gateways, not live services.
mod common;

use serde_json::{json, Value};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use trollm_core::{EventLog, EventType, ExecutionPlan, Executor, RunDir, SlotMap, TaskRegistry};
use trollm_models::MockModel;

fn record(seq: u64) -> Value {
    json!({"ts": 0, "runId": "run", "seq": seq, "type": "node_entered"})
}

#[test]
fn invalid_records_fail_closed_and_preserve_bytes() {
    let mut wrong_run = record(2);
    wrong_run["runId"] = json!("other");
    let mut bad_ts = record(2);
    bad_ts["ts"] = Value::Null;
    let mut bad_type = record(2);
    bad_type["type"] = json!("secret-unknown-type");
    let cases = [
        "{\"args\":\"secret-tool-argument\"".into(),
        "secret-tool-argument\n".into(),
        "[]".into(),
        "{}".into(),
        record(0).to_string(),
        record(1).to_string(),
        wrong_run.to_string(),
        bad_ts.to_string(),
        bad_type.to_string(),
    ];
    let base = common::tmp_dir("event-invalid");
    let path = base.join("events.jsonl");
    for tail in cases {
        let bytes = format!("{}\n{tail}", record(1)).into_bytes();
        std::fs::write(&path, &bytes).unwrap();
        let open_error = EventLog::open(&path, "run").err().unwrap();
        let read_error = EventLog::read_all(&path).unwrap_err();
        for error in [open_error, read_error] {
            assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
            let message = error.to_string();
            assert!(message.contains(&path.display().to_string()));
            assert!(message.contains("line 2"), "{message}");
            assert!(!message.contains("secret"), "{message}");
        }
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
    }
    // A decreasing sequence and invalid UTF-8 are also corrupt evidence.
    for bytes in [
        format!("{}\n{}\n", record(3), record(2)).into_bytes(),
        [record(1).to_string().as_bytes(), b"\n\xff"].concat(),
    ] {
        std::fs::write(&path, &bytes).unwrap();
        assert!(EventLog::open(&path, "run").is_err());
        assert!(EventLog::read_all(&path).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
    }
    std::fs::remove_dir_all(base).unwrap();
}

#[test]
fn complete_undelimited_tail_and_blank_lines_continue_without_loss() {
    let base = common::tmp_dir("event-delimiter");
    let path = base.join("events.jsonl");
    for suffix in ["", "\n", "\n \t", "\n\n"] {
        // Gaps are permitted by the monotonic sequence contract.
        let original = format!("\n \t\n{}\r\n{}{suffix}", record(1), record(7));
        std::fs::write(&path, &original).unwrap();
        let mut log = EventLog::open(&path, "run").unwrap();
        assert_eq!(log.last_seq(), 7);
        assert_eq!(std::fs::read(&path).unwrap(), original.as_bytes());
        assert_eq!(log.append(EventType::Resumed, json!({})).unwrap(), 8);
        drop(log);
        let mut reopened = EventLog::open(&path, "run").unwrap();
        assert_eq!(reopened.append(EventType::RunEnded, json!({})).unwrap(), 9);
        let events = EventLog::read_all(&path).unwrap();
        assert_eq!(
            events
                .iter()
                .map(|e| e["seq"].as_u64().unwrap())
                .collect::<Vec<_>>(),
            [1, 7, 8, 9]
        );
        assert!(std::fs::read(&path)
            .unwrap()
            .starts_with(original.as_bytes()));
    }
    std::fs::remove_dir_all(base).unwrap();
}

#[test]
fn expected_run_identity_reserved_payload_and_sequence_exhaustion_are_checked() {
    let base = common::tmp_dir("event-envelope");
    let path = base.join("events.jsonl");
    std::fs::write(&path, record(1).to_string()).unwrap();
    assert!(EventLog::open(&path, "other").is_err());
    let original = std::fs::read(&path).unwrap();
    let mut log = EventLog::open(&path, "run").unwrap();
    for key in ["ts", "runId", "seq", "type"] {
        let mut payload = json!({});
        payload[key] = json!("secret-value");
        let error = log.append(EventType::Error, payload).unwrap_err();
        assert!(!error.to_string().contains("secret-value"));
        assert_eq!(log.last_seq(), 1);
        assert_eq!(std::fs::read(&path).unwrap(), original);
    }
    drop(log);
    let original = record(u64::MAX).to_string();
    std::fs::write(&path, &original).unwrap();
    let mut log = EventLog::open(&path, "run").unwrap();
    assert!(log.append(EventType::Resumed, json!({})).is_err());
    assert_eq!(log.last_seq(), u64::MAX);
    assert_eq!(std::fs::read(&path).unwrap(), original.as_bytes());
    drop(log);
    std::fs::remove_dir_all(base).unwrap();
}

struct CountingGateway(Arc<AtomicUsize>);
impl trollm_mcp::ToolGateway for CountingGateway {
    fn tool_class(&self, _: &str, _: &str) -> trollm_mcp::policy::ToolClass {
        trollm_mcp::policy::ToolClass::Mutation
    }
    fn list_tools(&self) -> anyhow::Result<Vec<trollm_mcp::ToolDescriptor>> {
        Ok(vec![])
    }
    fn validate_call(&self, _: &trollm_mcp::ToolCall) -> anyhow::Result<()> {
        Ok(())
    }
    fn call(&mut self, _: &trollm_mcp::ToolCall) -> anyhow::Result<trollm_mcp::ToolResult> {
        self.0.fetch_add(1, Ordering::SeqCst);
        anyhow::bail!("corrupt journal must prevent dispatch")
    }
}

#[test]
fn torn_tail_and_corrupt_interior_stop_executor_before_dispatch_or_writes() {
    let base = common::tmp_dir("event-executor");
    std::fs::write(base.join("m.md"), "write").unwrap();
    let plan = ExecutionPlan::from_yaml_str(r#"
schemaVersion: ledger.execution-plan.v1
id: journal-integrity
version: 1
input: { promptTemplate: go }
limits: { maxSteps: 10, maxReasonerCalls: 0, maxTokensTotal: 100000, timeoutS: 600, artifactMaxMb: 5 }
onLimit: abort
models: { coder: { ctx: 8192 } }
entryStep: act
nodes:
  act: { kind: task, taskRef: mutate, onVerified: done }
  done: { kind: end, outcome: succeeded }
"#).unwrap();
    let registry_yaml = r#"
schemaVersion: ledger.task-registry.v1
tasks:
  mutate:
    model: coder
    prompt: m.md
    tools: [t.write]
    loop: { maxSteps: 3 }
    mutations: { idempotency: intent-log, onUncertain: fail-run }
    outputContract:
      outputs: { outcome: { from: model, type: prose } }
"#;
    for interior in [false, true] {
        let run = RunDir::under(&base);
        run.create_dirs().unwrap();
        let snapshot = trollm_core::RunState::fresh(run.run_id(), &plan.pin(), "act");
        let snapshot_bytes = serde_json::to_vec(&snapshot).unwrap();
        std::fs::write(run.snapshot_path(), &snapshot_bytes).unwrap();
        let mut first = record(1);
        first["runId"] = json!(run.run_id());
        let journal = format!(
            "{first}\n{{\"args\":\"secret-tool-argument\"{}",
            if interior {
                "\n".to_owned() + &first.to_string() + "\n"
            } else {
                String::new()
            }
        );
        std::fs::write(run.events_path(), &journal).unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let result = Executor::new(
            plan.clone(),
            TaskRegistry::from_yaml_str(registry_yaml, &base).unwrap(),
            String::new(),
            SlotMap::new().with(
                "coder",
                MockModel::scripted([
                    r#"<tool_call>{"server":"t","tool":"write","args":{}}</tool_call>"#,
                ]),
            ),
            CountingGateway(calls.clone()),
            run.clone(),
        );
        let error = result
            .err()
            .expect("journal corruption must prevent construction");
        let message = format!("{error:#}");
        assert!(message.contains("line 2"), "{message}");
        assert!(!message.contains("secret-tool-argument"));
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert_eq!(
            std::fs::read(run.events_path()).unwrap(),
            journal.as_bytes()
        );
        assert_eq!(std::fs::read(run.snapshot_path()).unwrap(), snapshot_bytes);
    }
    std::fs::remove_dir_all(base).unwrap();
}
