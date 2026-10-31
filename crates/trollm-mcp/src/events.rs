//! Proxy observations emitted through the gateway's event sink (§6).
//! The Android bridge persists them in `proxy.events.jsonl` with a separate
//! sequence; core does not currently translate them into main-log events.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ProxyEvent {
    /// Compact JSON UTF-8 sizes, never HTTP wire bytes. Stages describe the
    /// same data at different boundaries and must not be summed together.
    ResponseSize {
        server: String,
        tool: String,
        args_hash: String,
        dispatch_id: u64,
        stage: String,
        source: String,
        compact_json_utf8_bytes: Option<u64>,
        page: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        call_context: Option<crate::CallContext>,
        /// Cumulative phase milliseconds at this boundary. Subphases overlap
        /// total; absent phases were not entered. These are not wire timings.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timings: Option<serde_json::Value>,
    },
    CacheHit {
        server: String,
        tool: String,
        args_hash: String,
    },
    CacheMiss {
        server: String,
        tool: String,
        args_hash: String,
    },
    FetchStarted {
        server: String,
        tool: String,
        args_hash: String,
    },
    FetchProgress {
        server: String,
        tool: String,
        args_hash: String,
        pages: u32,
        items: u64,
    },
    FetchComplete {
        server: String,
        tool: String,
        args_hash: String,
        pages: u32,
        total_items: u64,
    },
    ServedFromFile {
        server: String,
        tool: String,
        fetched_by: String,
        match_value: String,
    },
    ShapedToFile {
        server: String,
        tool: String,
        field: String,
        path: PathBuf,
        bytes: u64,
    },
    /// Emitted BEFORE a mutation executes. Core writes its separate
    /// crash-safety intent record before dispatching to the gateway.
    MutationPassthrough {
        server: String,
        tool: String,
        args_hash: String,
    },
    LiveCall {
        server: String,
        tool: String,
        args_hash: String,
    },
    LocalCall {
        server: String,
        tool: String,
        args_hash: String,
    },
}

impl ProxyEvent {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::ResponseSize { .. } => "response_size",
            Self::CacheHit { .. } => "cache_hit",
            Self::CacheMiss { .. } => "cache_miss",
            Self::FetchStarted { .. } => "fetch_started",
            Self::FetchProgress { .. } => "fetch_progress",
            Self::FetchComplete { .. } => "fetch_complete",
            Self::ServedFromFile { .. } => "served_from_file",
            Self::ShapedToFile { .. } => "shaped_to_file",
            Self::MutationPassthrough { .. } => "mutation_passthrough",
            Self::LiveCall { .. } => "live_call",
            Self::LocalCall { .. } => "local_call",
        }
    }
}
