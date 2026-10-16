//! Serializable run state (§4.4). Persisted to `runs/<uuid>/state.snapshot.json`
//! at every node boundary (transition, frame push/pop, cursor advance) and on
//! pause. Resume and crash recovery are the identical path: load state,
//! continue dispatch; at most one node is re-executed.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// §4.4 `status` values.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StateStatus {
    Running,
    Paused,
    Succeeded,
    Failed,
}

/// One call frame (§4.4): pinned plan identity, current node, bound vars.
/// `nodeOutputs` and `counters` are core extensions of the §4.4 sketch (which
/// shows frames as `{plan, node, vars}` with a single top-level `counters`):
/// §4.4 gives each flowRef frame its "own counter scope", and the fixture
/// budgets only work that way (e.g. `return-to-list` allows maxSteps 2 per
/// invocation, `process-item` allows 1 reasoner call per item). The top-level
/// `RunState.counters` remains as the run-wide aggregate mirror of the sketch.
/// Frame counters persist across pause/resume (never reset); per-node loop
/// budgets reset on node entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Frame {
    /// `<planId>@sha256:<hex>`.
    pub plan: String,
    pub node: String,
    #[serde(default)]
    pub vars: Value,
    #[serde(default)]
    pub node_outputs: BTreeMap<String, BTreeMap<String, Value>>,
    #[serde(default)]
    pub counters: Counters,
}

/// Per-foreach cursor (§4.4 `foreachCursors`). v1 plans key these by node id
/// (the §4.4 JSON does the same); nested foreaches sharing a node id across
/// plans are out of scope for v1.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ForeachCursor {
    /// Index of the NEXT item to dispatch.
    pub index: usize,
    /// Path (relative to the run dir) of the immutable snapshot taken at entry
    /// (`enumeration: snapshot`, §3).
    pub snapshot_ref: String,
    /// Items processed since the run was last (re)started; compared against
    /// `maxItems`. Resets on resume — the documented compose-with-backgrounding
    /// behavior (PREMISE: "maxItems → paused; resume-from-cursor is the
    /// expected path"): after a pause/resume the run may process up to
    /// `maxItems` further items.
    #[serde(default)]
    pub session_items: u32,
    /// Partial per-item results; persist across `paused` (§4.6).
    #[serde(default)]
    pub results: Vec<Value>,
    /// Where in the body/between cycle this cursor sits.
    #[serde(default)]
    pub phase: ForeachPhase,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ForeachPhase {
    /// About to dispatch the next item (or finish).
    #[default]
    Next,
    /// Body frame is on the stack above us.
    InBody,
    /// Between frame is on the stack above us.
    InBetween,
}

/// Run-level counters (§4.4). Persist across pause/resume; never reset.
/// Per-node loop budgets live on the executor and reset on node entry.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Counters {
    #[serde(default)]
    pub steps: u32,
    #[serde(default)]
    pub reasoner_calls: u32,
    #[serde(default)]
    pub tokens: u64,
    #[serde(default)]
    pub started_at: u64,
    /// Bytes written to the run's artifact store (for `artifactMaxMb`).
    #[serde(default)]
    pub artifact_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingReturn {
    pub parent_plan: String,
    pub parent_node: String,
    pub outputs: BTreeMap<String, BTreeMap<String, Value>>,
    pub outcome: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunState {
    /// Log prefix committed together with this node/cursor boundary.
    #[serde(default)]
    pub event_watermark: u64,
    #[serde(default)]
    pub pending_return: Option<PendingReturn>,
    pub run_id: String,
    pub status: StateStatus,
    #[serde(default)]
    pub stack: Vec<Frame>,
    #[serde(default)]
    pub foreach_cursors: BTreeMap<String, ForeachCursor>,
    #[serde(default)]
    pub counters: Counters,
}

impl RunState {
    pub fn fresh(run_id: &str, entry_plan_pin: &str, entry_node: &str) -> Self {
        let now = crate::events::now_ms();
        Self {
            event_watermark: 0,
            pending_return: None,
            run_id: run_id.to_string(),
            status: StateStatus::Running,
            stack: vec![Frame {
                plan: entry_plan_pin.to_string(),
                node: entry_node.to_string(),
                vars: Value::Object(serde_json::Map::new()),
                node_outputs: BTreeMap::new(),
                counters: Counters {
                    started_at: now,
                    ..Counters::default()
                },
            }],
            foreach_cursors: BTreeMap::new(),
            counters: Counters {
                started_at: now,
                ..Counters::default()
            },
        }
    }
}
