//! Real journal thresholds with scripted mutation effects; no live services.
mod common;

use serde_json::{json, Value};
use std::fs::{self, File};
use std::io::{Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use trollm_core::events::{dangling_intents, now_ms, MAX_EVENT_LOG_BYTES, MAX_EVENT_RECORD_BYTES};
use trollm_core::{
    EventLog, EventType, ExecutionPlan, Executor, RunDir, RunStatus, SlotMap, TaskRegistry,
};
use trollm_mcp::{
    policy::ToolClass, Provenance, ToolCall, ToolDescriptor, ToolGateway, ToolResult,
};
use trollm_models::MockModel;

fn padded_record(size: usize) -> Vec<u8> {
    let mut value = json!({"ts":0,"runId":"run","seq":1,"type":"node_entered","pad":""});
    let overhead = serde_json::to_vec(&value).unwrap().len();
    value["pad"] = json!("x".repeat(size - overhead));
    let bytes = serde_json::to_vec(&value).unwrap();
    assert_eq!(bytes.len(), size);
    bytes
}

// Keep each blank line below the record limit, using a small reusable buffer.
fn blank_prefix(file: &mut File, mut count: u64) {
    let mut chunk = vec![b' '; 64 * 1024];
    while count != 0 {
        let length = count.min(chunk.len() as u64) as usize;
        chunk[length - 1] = b'\n';
        file.write_all(&chunk[..length]).unwrap();
        chunk[length - 1] = b' ';
        count -= length as u64;
    }
}

#[test]
fn record_boundaries_include_serialized_escaping_and_allow_undelimited_maximum() {
    let base = common::tmp_dir("record-bounds");
    let path = base.join("events.jsonl");
    for terminated in [false, true] {
        let mut bytes = padded_record(MAX_EVENT_RECORD_BYTES);
        if terminated {
            bytes.push(b'\n');
        }
        fs::write(&path, &bytes).unwrap();
        let mut log = EventLog::open(&path, "run").unwrap();
        assert_eq!(log.append(EventType::Resumed, json!({})).unwrap(), 2);
        assert_eq!(EventLog::visit(&path, Some("run"), |_, _| {}).unwrap(), 2);
    }
    for terminated in [false, true] {
        let mut bytes = padded_record(MAX_EVENT_RECORD_BYTES + 1);
        if terminated {
            bytes.push(b'\n');
        }
        fs::write(&path, &bytes).unwrap();
        assert!(EventLog::open(&path, "run")
            .err()
            .unwrap()
            .to_string()
            .contains("record exceeds"));
        assert!(EventLog::read_all(&path).is_err());
        assert_eq!(fs::read(&path).unwrap(), bytes);
    }
    fs::write(&path, b"").unwrap();
    let mut log = EventLog::open(&path, "run").unwrap();
    // Six serialized bytes per NUL, despite only one input byte per character.
    let fields = json!({"secret": "\0".repeat(MAX_EVENT_RECORD_BYTES / 6 + 1)});
    let error = log.append(EventType::Error, fields).unwrap_err();
    assert!(error.to_string().contains("record exceeds"));
    assert!(!error.to_string().contains("secret"));
    assert_eq!(
        log.append(EventType::Resumed, json!({}))
            .unwrap_err()
            .to_string(),
        error.to_string()
    );
    assert_eq!(log.last_seq(), 0);
    assert_eq!(fs::metadata(&path).unwrap().len(), 0);
    drop(log);
    fs::remove_dir_all(base).unwrap();
}

#[test]
fn append_accepts_exact_serialized_record_limit() {
    let base = common::tmp_dir("append-record-bound");
    let path = base.join("events.jsonl");
    let overhead =
        serde_json::to_vec(&json!({"ts":now_ms(),"runId":"run","seq":1,"type":"error","pad":""}))
            .unwrap()
            .len();
    let mut log = EventLog::open(&path, "run").unwrap();
    log.append(
        EventType::Error,
        json!({"pad":"x".repeat(MAX_EVENT_RECORD_BYTES - overhead)}),
    )
    .unwrap();
    assert_eq!(
        fs::metadata(&path).unwrap().len(),
        MAX_EVENT_RECORD_BYTES as u64 + 1
    );
    assert_eq!(EventLog::visit(&path, Some("run"), |_, _| {}).unwrap(), 1);
    drop(log);
    fs::remove_dir_all(base).unwrap();
}

#[test]
fn total_limit_counts_blanks_and_delimiter_before_any_write() {
    let base = common::tmp_dir("total-bounds");
    let path = base.join("events.jsonl");
    let record = padded_record(128);
    for terminated in [false, true] {
        let mut file = File::create(&path).unwrap();
        let ending = u64::from(terminated);
        blank_prefix(
            &mut file,
            MAX_EVENT_LOG_BYTES - record.len() as u64 - ending,
        );
        file.write_all(&record).unwrap();
        if terminated {
            file.write_all(b"\n").unwrap();
        }
        drop(file);
        let mut log = EventLog::open(&path, "run").unwrap();
        let error = log.append(EventType::Resumed, json!({})).unwrap_err();
        assert!(error.to_string().contains("log exceeds"));
        assert_eq!(log.last_seq(), 1);
        assert_eq!(fs::metadata(&path).unwrap().len(), MAX_EVENT_LOG_BYTES);
        let mut file = File::open(&path).unwrap();
        file.seek(SeekFrom::End(-(record.len() as i64 + ending as i64)))
            .unwrap();
        let mut tail = Vec::new();
        std::io::Read::read_to_end(&mut file, &mut tail).unwrap();
        assert_eq!(
            tail,
            [record.as_slice(), if terminated { b"\n" } else { b"" }].concat()
        );
    }
    // Appending may reach the physical cap exactly (including the new LF).
    let overhead =
        serde_json::to_vec(&json!({"ts":now_ms(),"runId":"run","seq":1,"type":"error","pad":""}))
            .unwrap()
            .len();
    let mut file = File::create(&path).unwrap();
    blank_prefix(&mut file, MAX_EVENT_LOG_BYTES - overhead as u64 - 1);
    drop(file);
    let mut log = EventLog::open(&path, "run").unwrap();
    assert_eq!(log.append(EventType::Error, json!({"pad":""})).unwrap(), 1);
    assert_eq!(fs::metadata(&path).unwrap().len(), MAX_EVENT_LOG_BYTES);
    drop(log);

    // A sparse oversized file rejects by metadata before invalid NULs are parsed.
    File::create(&path)
        .unwrap()
        .set_len(MAX_EVENT_LOG_BYTES + 1)
        .unwrap();
    assert!(EventLog::open(&path, "run")
        .err()
        .unwrap()
        .to_string()
        .contains("log exceeds"));
    assert!(EventLog::visit(&path, None, |_, _| {})
        .unwrap_err()
        .to_string()
        .contains("log exceeds"));
    assert_eq!(fs::metadata(&path).unwrap().len(), MAX_EVENT_LOG_BYTES + 1);
    fs::remove_dir_all(base).unwrap();
}

const PLAN: &str = r#"
schemaVersion: ledger.execution-plan.v1
id: journal-capacity
version: 1
limits: { maxSteps: 10, maxReasonerCalls: 0, maxTokensTotal: 100000, timeoutS: 600, artifactMaxMb: 1 }
onLimit: abort
models: { coder: { ctx: 2048 } }
entryStep: work
nodes:
  work: { kind: task, taskRef: work, onVerified: done }
  done: { kind: end, outcome: succeeded }
"#;
const REGISTRY: &str = r#"
schemaVersion: ledger.task-registry.v1
tasks:
  work:
    model: coder
    prompt: work.md
    tools: [t.write]
    loop: { maxSteps: 2 }
    mutations: { idempotency: intent-log, onUncertain: fail-run }
    outputContract:
      outputs: { report: { from: model, type: prose } }
"#;
const FENCE: &str = r#"<tool_call>{"server":"t","tool":"write","args":{}}</tool_call>"#;

struct Effects(Arc<AtomicUsize>);
impl ToolGateway for Effects {
    fn tool_class(&self, _: &str, _: &str) -> ToolClass {
        ToolClass::Mutation
    }
    fn list_tools(&self) -> anyhow::Result<Vec<ToolDescriptor>> {
        Ok(vec![])
    }
    fn validate_call(&self, _: &ToolCall) -> anyhow::Result<()> {
        Ok(())
    }
    fn call(&mut self, _: &ToolCall) -> anyhow::Result<ToolResult> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(ToolResult {
            content_for_model: json!({"ok":true}),
            artifacts: vec![],
            provenance: Provenance::Live,
        })
    }
}
fn executor(base: &Path, run: RunDir, effects: Arc<AtomicUsize>) -> Executor<MockModel, Effects> {
    Executor::new(
        ExecutionPlan::from_yaml_str(PLAN).unwrap(),
        TaskRegistry::from_yaml_str(REGISTRY, base).unwrap(),
        String::new(),
        SlotMap::new().with("coder", MockModel::scripted([FENCE, "done"])),
        Effects(effects),
        run,
    )
    .unwrap()
}

#[test]
fn capacity_blocks_intent_dispatch_and_preserves_uncertainty_after_effect() {
    let base = common::tmp_dir("mutation-capacity");
    fs::write(base.join("work.md"), "write once").unwrap();
    // Measure the real small event prefix instead of depending on private test limits.
    let baseline = RunDir::under(&base);
    let mut normal = executor(&base, baseline.clone(), Arc::new(AtomicUsize::new(0)));
    assert_eq!(normal.run_until_blocked().unwrap(), RunStatus::Succeeded);
    drop(normal);
    let journal = fs::read_to_string(baseline.events_path()).unwrap();
    let mut prefix = 0;
    let mut before_intent = 0;
    let mut through_intent = 0;
    let mut through_result = 0;
    for line in journal.split_inclusive('\n') {
        let event: Value = serde_json::from_str(line).unwrap();
        if event["type"] == "tool_call_intent" {
            before_intent = prefix;
            through_intent = prefix + line.len() as u64;
        }
        prefix += line.len() as u64;
        if event["type"] == "tool_call_result" {
            through_result = prefix;
            break;
        }
    }
    // Midpoints leave wide margins on either side of the refused record;
    // scheduler-dependent timing digits cannot decide whether the effect runs.
    assert!(through_intent - before_intent >= 128);
    assert!(through_result - through_intent >= 128);
    for after_effect in [false, true] {
        let run = RunDir::under(&base);
        run.create_dirs().unwrap();
        let allowance = if after_effect {
            through_intent + (through_result - through_intent) / 2
        } else {
            before_intent + (through_intent - before_intent) / 2
        };
        let mut file = File::create(run.events_path()).unwrap();
        blank_prefix(&mut file, MAX_EVENT_LOG_BYTES - allowance);
        drop(file);
        let effects = Arc::new(AtomicUsize::new(0));
        let mut exec = executor(&base, run.clone(), effects.clone());
        assert!(exec
            .run_until_blocked()
            .unwrap_err()
            .to_string()
            .contains("log exceeds"));
        assert_eq!(effects.load(Ordering::SeqCst), usize::from(after_effect));
        drop(exec);
        let events = EventLog::read_all(&run.events_path()).unwrap();
        assert_eq!(dangling_intents(&events).len(), usize::from(after_effect));
        let size = fs::metadata(run.events_path()).unwrap().len();
        assert!(size <= MAX_EVENT_LOG_BYTES);
        if after_effect {
            for _ in 0..2 {
                let mut restored = executor(&base, run.clone(), effects.clone());
                assert!(restored.resume().is_err());
                assert_eq!(effects.load(Ordering::SeqCst), 1);
                assert_eq!(fs::metadata(run.events_path()).unwrap().len(), size);
            }
        }
    }
    fs::remove_dir_all(base).unwrap();
}
