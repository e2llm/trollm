//! Append-only event log (§7): `runs/<uuid>/events.jsonl`, one JSON object per
//! line, monotonic `seq` per run. The log is the audit trail, the replay
//! mechanism, and the crash-recovery source for mutation journaling (§6).

use serde::ser::SerializeMap;
use serde::{Deserialize, Serialize, Serializer};
use serde_json::Value;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

/// Disposable observation of a committed record. Never used for execution or recovery.
pub type CommittedEventSink = Arc<dyn Fn(&Value, &str) + Send + Sync>;

/// Closed set of main-log event types (§7). `fetch_progress`, `cache_hit` and
/// `served_from_file` are reserved here; core currently does not emit them.
/// The Android bridge writes gateway observations to a separately sequenced
/// `proxy.events.jsonl` sidecar, outside this log.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventType {
    RunStarted,
    PlanLoaded,
    NodeEntered,
    NodeExited,
    ModelCall,
    ToolCallPrepared,
    ToolCallOutcomeAcknowledged,
    ToolCallIntent,
    ToolCallResult,
    CacheHit,
    ServedFromFile,
    ArtifactWritten,
    FetchProgress,
    Transition,
    BudgetExhausted,
    Paused,
    Resumed,
    Error,
    RunEnded,
}

impl EventType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::RunStarted => "run_started",
            Self::PlanLoaded => "plan_loaded",
            Self::NodeEntered => "node_entered",
            Self::NodeExited => "node_exited",
            Self::ModelCall => "model_call",
            Self::ToolCallPrepared => "tool_call_prepared",
            Self::ToolCallOutcomeAcknowledged => "tool_call_outcome_acknowledged",
            Self::ToolCallIntent => "tool_call_intent",
            Self::ToolCallResult => "tool_call_result",
            Self::CacheHit => "cache_hit",
            Self::ServedFromFile => "served_from_file",
            Self::ArtifactWritten => "artifact_written",
            Self::FetchProgress => "fetch_progress",
            Self::Transition => "transition",
            Self::BudgetExhausted => "budget_exhausted",
            Self::Paused => "paused",
            Self::Resumed => "resumed",
            Self::Error => "error",
            Self::RunEnded => "run_ended",
        }
    }
}

/// Serialized JSON bytes per event, excluding its LF delimiter.
pub const MAX_EVENT_RECORD_BYTES: usize = 1024 * 1024;
/// Physical journal bytes, including delimiters and blank lines.
pub const MAX_EVENT_LOG_BYTES: u64 = 32 * 1024 * 1024;

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Cumulative successful main-journal append costs; intervals are disjoint.
/// Nanoseconds saturate rather than wrapping. No payloads or identifiers.
#[derive(Debug, Default, Clone, Copy)]
pub struct EventWriteMetrics {
    pub records: u64,
    /// Serialized record bytes, excluding delimiters.
    pub record_bytes: u64,
    pub serialize_ns: u64,
    pub write_flush_ns: u64,
    pub observer_ns: u64,
}

fn elapsed_ns(start: Instant) -> u64 {
    start.elapsed().as_nanos().min(u64::MAX as u128) as u64
}

/// Append-only JSONL writer. Every line is flushed before returning so mutation
/// intents precede gateway dispatch. This is not an fsync/power-loss guarantee.
pub struct EventLog {
    path: PathBuf,
    file: File,
    run_id: String,
    seq: u64,
    needs_delimiter: bool,
    bytes: u64,
    failure: Option<(std::io::ErrorKind, String)>,
    committed_sink: Option<CommittedEventSink>,
    write_metrics: EventWriteMetrics,
}

impl EventLog {
    /// Open (or create) the log for `run_id`. On an existing log, `seq`
    /// continues after the last recorded line (resume path, §4.4).
    pub fn open(path: &Path, run_id: &str) -> std::io::Result<Self> {
        let (seq, needs_delimiter, bytes) = scan(path, Some(run_id), |_, _| {})?;
        let file = OpenOptions::new().create(true).append(true).open(path)?;
        Ok(Self {
            path: path.to_path_buf(),
            file,
            run_id: run_id.to_string(),
            seq,
            needs_delimiter,
            bytes,
            failure: None,
            committed_sink: None,
            write_metrics: EventWriteMetrics::default(),
        })
    }

    /// Install a best-effort observer. Callback failures cannot change append success,
    /// mutation ordering or recovery. The callback must not perform execution work.
    pub fn set_committed_sink(&mut self, sink: CommittedEventSink) {
        self.committed_sink = Some(sink);
    }

    pub fn write_metrics(&self) -> EventWriteMetrics {
        self.write_metrics
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn last_seq(&self) -> u64 {
        self.seq
    }

    /// Append one event; returns its seq. `fields` are the type-specific
    /// payload merged into the envelope `{ts, runId, seq, type}`.
    pub fn append(&mut self, ty: EventType, fields: Value) -> std::io::Result<u64> {
        if let Some((kind, detail)) = &self.failure {
            return Err(std::io::Error::new(*kind, detail.clone()));
        }
        let result = self.append_record(ty, fields);
        if let Err(error) = &result {
            self.failure = Some((error.kind(), error.to_string()));
        }
        result
    }

    fn append_record(&mut self, ty: EventType, fields: Value) -> std::io::Result<u64> {
        if let Value::Object(extra) = &fields {
            for key in ["ts", "runId", "seq", "type"] {
                if extra.contains_key(key) {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        format!("event payload cannot override envelope field {key}"),
                    ));
                }
            }
        }
        let next_seq = self.seq.checked_add(1).ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, "event sequence exhausted")
        })?;
        let serialize_started = Instant::now();
        let mut record = RecordBuffer(Vec::new());
        let mut serializer = serde_json::Serializer::new(&mut record);
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("ts", &now_ms())?;
        map.serialize_entry("runId", &self.run_id)?;
        map.serialize_entry("seq", &next_seq)?;
        map.serialize_entry("type", ty.as_str())?;
        if let Value::Object(extra) = &fields {
            for (key, value) in extra {
                map.serialize_entry(key, value)?;
            }
        }
        map.end()?;
        let serialize_ns = elapsed_ns(serialize_started);
        let next_bytes = self.bytes + u64::from(self.needs_delimiter) + record.0.len() as u64 + 1;
        if next_bytes > MAX_EVENT_LOG_BYTES {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "event log exceeds byte limit",
            ));
        }
        // Capacity rejection above must not even delimit a previously valid tail.
        let write_started = Instant::now();
        if self.needs_delimiter {
            self.file.write_all(b"\n")?;
        }
        self.file.write_all(&record.0)?;
        self.file.write_all(b"\n")?;
        self.file.flush()?;
        let write_flush_ns = elapsed_ns(write_started);
        self.needs_delimiter = false;
        self.bytes = next_bytes;
        self.seq = next_seq;
        self.write_metrics.records = self.write_metrics.records.saturating_add(1);
        self.write_metrics.record_bytes = self
            .write_metrics
            .record_bytes
            .saturating_add(record.0.len() as u64);
        self.write_metrics.serialize_ns =
            self.write_metrics.serialize_ns.saturating_add(serialize_ns);
        self.write_metrics.write_flush_ns = self
            .write_metrics
            .write_flush_ns
            .saturating_add(write_flush_ns);
        if let Some(sink) = &self.committed_sink {
            let observer_started = Instant::now();
            // The serialized record is already committed. A disposable projection
            // must never convert this success into a failed mutation intent.
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                if let (Ok(value), Ok(line)) = (
                    serde_json::from_slice::<Value>(&record.0),
                    std::str::from_utf8(&record.0),
                ) {
                    sink(&value, line);
                }
            }));
            self.write_metrics.observer_ns = self
                .write_metrics
                .observer_ns
                .saturating_add(elapsed_ns(observer_started));
        }
        Ok(self.seq)
    }

    /// Visit records in sequence without retaining the journal in memory.
    /// The raw line excludes LF and an optional preceding CR. All records are
    /// validated; callers must discard accumulated results if this returns Err.
    pub fn visit(
        path: &Path,
        expected_run_id: Option<&str>,
        visitor: impl FnMut(&Value, &str),
    ) -> std::io::Result<u64> {
        scan(path, expected_run_id, visitor).map(|(seq, _, _)| seq)
    }

    /// Compatibility collector. Prefer visit for recovery and status scans.
    pub fn read_all(path: &Path) -> std::io::Result<Vec<Value>> {
        let mut out = Vec::new();
        Self::visit(path, None, |event, _| out.push(event.clone()))?;
        Ok(out)
    }
}

/// Exact retained JSON record text (without JSONL delimiters), not regenerated
/// native events and not a cryptographic signature or business disposition.
pub struct RetainedCallRecords {
    pub intent_json: String,
    pub result_json: String,
}

/// Validate the original selected preparation and its later retained intent
/// before a dispatch permit exists. The caller supplies the native-owned path.
pub fn retained_prepared_intent(
    path: &Path,
    context: &trollm_mcp::preparation::PreparationContext,
    call: &trollm_mcp::ToolCall,
    evidence: &trollm_mcp::preparation::PreparationEvidence,
    intent_seq: u64,
) -> anyhow::Result<(String, String)> {
    use sha2::{Digest, Sha256};
    let native = serde_json::to_value(&context.call_context)?;
    let mut intent = None;
    let mut prepared = None;
    let mut duplicate = false;
    EventLog::visit(path, Some(&context.run_id), |event, raw| {
        if event["seq"].as_u64() == Some(intent_seq) {
            intent = Some((event.clone(), raw.to_owned()));
        }
        if event["type"] == "tool_call_prepared" && event["callContext"] == native {
            duplicate |= prepared.is_some();
            prepared = Some((event.clone(), raw.to_owned()));
        }
    })?;
    anyhow::ensure!(!duplicate, "selected native preparation duplicated");
    let (prepared, prepared_raw) =
        prepared.ok_or_else(|| anyhow::anyhow!("selected native preparation unavailable"))?;
    let (intent, raw) =
        intent.ok_or_else(|| anyhow::anyhow!("selected retained intent unavailable"))?;
    anyhow::ensure!(raw.len() <= 16384, "selected intent exceeds evidence bound");
    for record in [&prepared, &intent] {
        anyhow::ensure!(
            record["node"] == context.node
                && record["taskRef"] == context.task_ref
                && record["callContext"] == native,
            "selected native intent context mismatch"
        );
    }
    anyhow::ensure!(
        prepared["seq"].as_u64().is_some_and(|seq| seq < intent_seq)
            && prepared["resolvedCall"] == serde_json::to_value(call)?
            && prepared["nativeContext"] == serde_json::to_value(context.raw()?)?
            && prepared["evidence"] == serde_json::to_value(evidence)?,
        "selected intent does not follow its original preparation"
    );
    anyhow::ensure!(
        intent["type"] == "tool_call_intent"
            && intent["server"] == call.server
            && intent["tool"] == call.tool
            && intent["args"] == call.args
            && intent["argsSha256"]
                == format!("{:x}", Sha256::digest(serde_json::to_vec(&call.args)?)),
        "selected retained intent arguments mismatch"
    );
    Ok((raw, prepared_raw))
}

/// Extract from the native-owned journal only after validating its complete
/// run/sequence stream. External callers must not select an arbitrary journal.
/// Artifact bytes and definite tool disposition require additional validation.
pub fn retained_call_records(
    path: &Path,
    context: &trollm_mcp::preparation::PreparationContext,
    call: &trollm_mcp::ToolCall,
    intent_seq: u64,
    result_seq: u64,
) -> anyhow::Result<RetainedCallRecords> {
    anyhow::ensure!(intent_seq < result_seq, "invalid retained call order");
    let mut intent = None;
    let mut result = None;
    EventLog::visit(path, Some(&context.run_id), |event, raw| {
        if event["seq"].as_u64() == Some(intent_seq) {
            intent = Some((event.clone(), raw.to_owned()));
        }
        if event["seq"].as_u64() == Some(result_seq) {
            result = Some((event.clone(), raw.to_owned()));
        }
    })?;
    let (intent, intent_json) =
        intent.ok_or_else(|| anyhow::anyhow!("retained intent unavailable"))?;
    let (result, result_json) =
        result.ok_or_else(|| anyhow::anyhow!("retained result unavailable"))?;
    anyhow::ensure!(
        intent_json.len() + result_json.len() <= 16384,
        "retained call evidence exceeds selected bound"
    );
    let native_context = serde_json::to_value(&context.call_context)?;
    for event in [&intent, &result] {
        anyhow::ensure!(
            event["node"] == context.node
                && event["taskRef"] == context.task_ref
                && event["server"] == call.server
                && event["tool"] == call.tool
                && event["callContext"] == native_context,
            "retained call association mismatch"
        );
    }
    anyhow::ensure!(
        intent["type"] == "tool_call_intent" && intent["args"] == call.args,
        "retained intent arguments mismatch"
    );
    anyhow::ensure!(
        result["type"] == "tool_call_result"
            && result["intentSeq"].as_u64() == Some(intent_seq)
            && result["argsHash"] == trollm_mcp::CacheStore::hash_args(&call.args, None, None),
        "retained result linkage mismatch"
    );
    Ok(RetainedCallRecords {
        intent_json,
        result_json,
    })
}

/// Serialization stops before an oversized event can allocate its full JSON.
struct RecordBuffer(Vec<u8>);

impl Write for RecordBuffer {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > MAX_EVENT_RECORD_BYTES - self.0.len() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "event record exceeds byte limit",
            ));
        }
        if self.0.capacity() - self.0.len() < bytes.len() {
            let capacity = (self.0.capacity().saturating_mul(2))
                .max(self.0.len() + bytes.len())
                .min(MAX_EVENT_RECORD_BYTES);
            self.0.reserve_exact(capacity - self.0.len());
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Validate the complete journal before opening a writer or exposing recovery
/// events. Sequence gaps are allowed; duplicates and decreases are not.
/// Diagnostics deliberately omit record contents (which can contain secrets).
fn scan(
    path: &Path,
    expected_run_id: Option<&str>,
    mut event: impl FnMut(&Value, &str),
) -> std::io::Result<(u64, bool, u64)> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok((0, false, 0)),
        Err(error) => return Err(error),
    };
    if file.metadata()?.len() > MAX_EVENT_LOG_BYTES {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("{}: event log exceeds byte limit", path.display()),
        ));
    }
    let mut reader = BufReader::new(file);
    let mut buffer = Vec::with_capacity(MAX_EVENT_RECORD_BYTES + 1);
    let mut bytes = 0u64;
    let mut line_number = 0;
    let mut seq = 0;
    let mut run_id = expected_run_id.map(str::to_owned);
    let mut needs_delimiter = false;
    loop {
        buffer.clear();
        let count = reader
            .by_ref()
            .take(((MAX_EVENT_RECORD_BYTES + 1) as u64).min(MAX_EVENT_LOG_BYTES - bytes + 1))
            .read_until(b'\n', &mut buffer)?;
        if count == 0 {
            break;
        }
        line_number += 1;
        let invalid = |reason| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("{}: line {line_number}: {reason}", path.display()),
            )
        };
        bytes += count as u64;
        if bytes > MAX_EVENT_LOG_BYTES {
            return Err(invalid("event log exceeds byte limit"));
        }
        needs_delimiter = !buffer.ends_with(b"\n");
        let content = if needs_delimiter {
            &buffer[..]
        } else {
            &buffer[..buffer.len() - 1]
        };
        if content.len() > MAX_EVENT_RECORD_BYTES {
            return Err(invalid("event record exceeds byte limit"));
        }
        let line = std::str::from_utf8(content).map_err(|_| invalid("invalid UTF-8"))?;
        let line = if needs_delimiter {
            line
        } else {
            line.strip_suffix('\r').unwrap_or(line)
        };
        // Empty/whitespace-only lines carry no event, including at EOF.
        if line.trim().is_empty() {
            continue;
        }
        let value: Value =
            serde_json::from_str(line).map_err(|_| invalid("malformed JSON record"))?;
        let object = value
            .as_object()
            .ok_or_else(|| invalid("event must be an object"))?;
        if !object.get("ts").is_some_and(Value::is_number) {
            return Err(invalid("ts must be numeric"));
        }
        if object
            .get("type")
            .cloned()
            .and_then(|ty| serde_json::from_value::<EventType>(ty).ok())
            .is_none()
        {
            return Err(invalid("type must be a recognized event type"));
        }
        let record_run_id = object
            .get("runId")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
            .ok_or_else(|| invalid("runId must be a nonempty string"))?;
        if let Some(expected) = &run_id {
            if expected != record_run_id {
                return Err(invalid("runId does not match the journal run"));
            }
        } else {
            run_id = Some(record_run_id.to_owned());
        }
        let next_seq = object
            .get("seq")
            .and_then(Value::as_u64)
            .filter(|seq| *seq > 0)
            .ok_or_else(|| invalid("seq must be a positive integer"))?;
        if next_seq <= seq {
            return Err(invalid("seq must be strictly increasing"));
        }
        seq = next_seq;
        event(&value, line);
    }
    Ok((seq, needs_delimiter, bytes))
}

/// A `tool_call_intent` with no matching `tool_call_result` (§6 mutation
/// journaling crash-recovery scan).
#[derive(Debug, Clone)]
pub struct DanglingIntent {
    pub seq: u64,
    pub node: String,
    pub task_ref: String,
    pub server: String,
    pub tool: String,
    pub args: Value,
}

impl DanglingIntent {
    pub(crate) fn from_event(seq: u64, e: &Value) -> Self {
        Self {
            seq,
            node: e
                .get("node")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            task_ref: e
                .get("taskRef")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            server: e
                .get("server")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            tool: e
                .get("tool")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            args: e.get("args").cloned().unwrap_or(Value::Null),
        }
    }
}

/// Scan a log for intents that never got a result. Results pair to intents via
/// the `intentSeq` field core stamps on `tool_call_result` events.
pub fn dangling_intents(events: &[Value]) -> Vec<DanglingIntent> {
    let mut intents: Vec<(u64, &Value)> = Vec::new();
    let mut answered = std::collections::BTreeSet::new();
    for e in events {
        match e.get("type").and_then(|t| t.as_str()) {
            Some("tool_call_intent") => {
                if let Some(s) = e.get("seq").and_then(|s| s.as_u64()) {
                    intents.push((s, e));
                }
            }
            Some("tool_call_result") => {
                if let Some(s) = e.get("intentSeq").and_then(|s| s.as_u64()) {
                    answered.insert(s);
                }
            }
            _ => {}
        }
    }
    intents
        .into_iter()
        .filter(|(s, _)| !answered.contains(s))
        .map(|(seq, event)| DanglingIntent::from_event(seq, event))
        .collect()
}

#[cfg(test)]
mod bounds_regressions {
    use super::*;

    #[test]
    fn retained_call_extraction_preserves_native_bytes_and_refuses_association_or_corrupt_tail() {
        use serde_json::json;
        let directory =
            std::env::temp_dir().join(format!("retained-call-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&directory).unwrap();
        let path = directory.join("events.jsonl");
        let context = trollm_mcp::preparation::PreparationContext {
            run_id: "run".into(),
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
            args: json!({"id":3}),
        };
        let mut log = EventLog::open(&path, "run").unwrap();
        let common = json!({"node":"node","taskRef":"task","server":"fixture","tool":"act","callContext":context.call_context});
        let mut intent = common.clone();
        intent["args"] = call.args.clone();
        let first = log.append(EventType::ToolCallIntent, intent).unwrap();
        let mut result = common;
        result["intentSeq"] = json!(first);
        result["argsHash"] = json!(trollm_mcp::CacheStore::hash_args(&call.args, None, None));
        let last = log.append(EventType::ToolCallResult, result).unwrap();
        let retained = retained_call_records(&path, &context, &call, first, last).unwrap();
        let original = std::fs::read_to_string(&path).unwrap();
        assert_eq!(
            original,
            format!("{}\n{}\n", retained.intent_json, retained.result_json)
        );
        assert!(retained.intent_json.contains("18446744073709551615"));
        let mut wrong = call.clone();
        wrong.args = json!({"id":4});
        assert!(retained_call_records(&path, &context, &wrong, first, last).is_err());
        assert!(retained_call_records(&path, &context, &call, last, first).is_err());
        drop(log);
        OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"{broken\n")
            .unwrap();
        assert!(retained_call_records(&path, &context, &call, first, last).is_err());
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn committed_observer_is_post_flush_and_panics_do_not_change_success() {
        let directory =
            std::env::temp_dir().join(format!("event-observer-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&directory).unwrap();
        let path = directory.join("events.jsonl");
        let mut log = EventLog::open(&path, "run").unwrap();
        let observe_path = path.clone();
        log.set_committed_sink(Arc::new(move |value, line| {
            assert_eq!(value["seq"], 1);
            assert_eq!(
                std::fs::read_to_string(&observe_path).unwrap(),
                format!("{line}\n")
            );
            panic!("disposable observer failure");
        }));
        assert_eq!(log.append(EventType::Resumed, Value::Null).unwrap(), 1);
        let metrics = log.write_metrics();
        assert_eq!(metrics.records, 1);
        assert_eq!(
            metrics.record_bytes + 1,
            std::fs::metadata(&path).unwrap().len()
        );
        log.file = File::open(&path).unwrap();
        assert!(log.append(EventType::Resumed, Value::Null).is_err());
        assert_eq!(log.write_metrics().records, 1);
        drop(log);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn failed_io_stays_failed_even_when_file_is_replaced_with_writable_handle() {
        let directory =
            std::env::temp_dir().join(format!("event-io-failure-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&directory).unwrap();
        let path = directory.join("events.jsonl");
        let mut log = EventLog::open(&path, "run").unwrap();
        log.file = File::open(&path).unwrap(); // force a real write error
        let error = log.append(EventType::Resumed, Value::Null).unwrap_err();
        log.file = OpenOptions::new().append(true).open(&path).unwrap();
        let repeated = log.append(EventType::RunEnded, Value::Null).unwrap_err();
        assert_eq!(repeated.kind(), error.kind());
        assert_eq!(repeated.to_string(), error.to_string());
        assert_eq!(log.last_seq(), 0);
        assert_eq!(std::fs::metadata(&path).unwrap().len(), 0);
        drop(log);
        std::fs::remove_dir_all(directory).unwrap();
    }
}
