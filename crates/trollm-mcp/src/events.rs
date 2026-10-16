//! Proxy events emitted by the gateway (§6: "every proxy decision is an
//! event-log line"). Dependency-free: core translates these into its own
//! event-log types.

use serde::Serialize;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ProxyEvent {
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
    /// Emitted BEFORE a mutation executes (core journals this as the
    /// crash-safety intent record).
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
}

impl ProxyEvent {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::CacheHit { .. } => "cache_hit",
            Self::CacheMiss { .. } => "cache_miss",
            Self::FetchStarted { .. } => "fetch_started",
            Self::FetchProgress { .. } => "fetch_progress",
            Self::FetchComplete { .. } => "fetch_complete",
            Self::ServedFromFile { .. } => "served_from_file",
            Self::ShapedToFile { .. } => "shaped_to_file",
            Self::MutationPassthrough { .. } => "mutation_passthrough",
            Self::LiveCall { .. } => "live_call",
        }
    }
}
