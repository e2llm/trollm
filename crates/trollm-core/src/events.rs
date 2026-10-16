//! Append-only event log (§7): `runs/<uuid>/events.jsonl`, one JSON object per
//! line, monotonic `seq` per run. The log is the audit trail, the replay
//! mechanism, and the crash-recovery source for mutation journaling (§6).

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Closed set of event types (§7). `fetch_progress`, `cache_hit` and
/// `served_from_file` are emitted by the gateway (trollm-mcp) via its sink;
/// core accepts and sequences them identically.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventType {
    RunStarted,
    PlanLoaded,
    NodeEntered,
    NodeExited,
    ModelCall,
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

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Append-only JSONL writer. Every line is flushed immediately: the log must
/// survive a process crash at any point (§6 crash safety depends on a durable
/// `tool_call_intent` hitting disk before the gateway call happens).
pub struct EventLog {
    path: PathBuf,
    file: File,
    run_id: String,
    seq: u64,
}

impl EventLog {
    /// Open (or create) the log for `run_id`. On an existing log, `seq`
    /// continues after the last recorded line (resume path, §4.4).
    pub fn open(path: &Path, run_id: &str) -> std::io::Result<Self> {
        let mut seq = 0u64;
        if path.exists() {
            for line in BufReader::new(File::open(path)?).lines() {
                let line = line?;
                if line.trim().is_empty() {
                    continue;
                }
                if let Ok(v) = serde_json::from_str::<Value>(&line) {
                    if let Some(s) = v.get("seq").and_then(|s| s.as_u64()) {
                        seq = seq.max(s);
                    }
                }
            }
        }
        let file = OpenOptions::new().create(true).append(true).open(path)?;
        Ok(Self {
            path: path.to_path_buf(),
            file,
            run_id: run_id.to_string(),
            seq,
        })
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
        self.seq += 1;
        let mut obj = json!({
            "ts": now_ms(),
            "runId": self.run_id,
            "seq": self.seq,
            "type": ty.as_str(),
        });
        if let (Value::Object(base), Value::Object(extra)) = (&mut obj, &fields) {
            for (k, v) in extra {
                base.insert(k.clone(), v.clone());
            }
        }
        let mut line = serde_json::to_string(&obj).unwrap_or_else(|_| "{}".to_string());
        line.push('\n');
        self.file.write_all(line.as_bytes())?;
        self.file.flush()?;
        Ok(self.seq)
    }

    /// Read the whole log back (resume / crash-recovery scan).
    pub fn read_all(path: &Path) -> std::io::Result<Vec<Value>> {
        let mut out = Vec::new();
        if !path.exists() {
            return Ok(out);
        }
        for line in BufReader::new(File::open(path)?).lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            if let Ok(v) = serde_json::from_str::<Value>(&line) {
                out.push(v);
            }
        }
        Ok(out)
    }
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
        .map(|(s, e)| DanglingIntent {
            seq: s,
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
        })
        .collect()
}
