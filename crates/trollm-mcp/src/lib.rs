//! trollm-mcp: the ToolGateway port + (implemented per
//! docs/ARCHITECTURE-ANDROID.md §6,§9) McpGateway with the policy engine.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub mod cache;
pub mod events;
pub mod gateway;
pub mod policy;
mod schema;
pub mod transport;

pub use cache::{CacheMeta, CacheStore};
pub use events::ProxyEvent;
pub use gateway::{EventSink, McpGateway, ServerConfig};
pub use policy::{PolicySet, ToolPolicy};
pub use transport::{RootCertificates, TokenGetter};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDescriptor {
    pub server: String,
    pub name: String,
    pub input_schema: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub server: String,
    pub tool: String,
    pub args: serde_json::Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Provenance {
    Live,
    Cache,
    ServedFromFile,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    pub content_for_model: serde_json::Value,
    pub artifacts: Vec<PathBuf>,
    pub provenance: Provenance,
}

pub trait ToolGateway: Send {
    fn list_tools(&self) -> Result<Vec<ToolDescriptor>>;
    /// Effective policy classification for this gateway. Must be stable for
    /// the run and must not dispatch tools. Adapters must forward it so core
    /// can enforce mutation journaling even when plan and gateway differ.
    fn tool_class(&self, server: &str, tool: &str) -> policy::ToolClass;
    /// Validate a granted call before recording mutation intent. This must not
    /// dispatch tools. Concrete gateways also protect their direct call path.
    fn validate_call(&self, call: &ToolCall) -> Result<()>;
    /// Executes a tool call, applying gateway policies (cache/paginate/serveFrom/
    /// shape/mutation). Implementations must be the ONLY path models have to MCP.
    fn call(&mut self, call: &ToolCall) -> Result<ToolResult>;
}
