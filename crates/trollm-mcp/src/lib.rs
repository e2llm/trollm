//! trollm-mcp: the ToolGateway port + (implemented per
//! docs/ARCHITECTURE-ANDROID.md §6,§9) McpGateway with the policy engine.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub mod cache;
pub mod events;
pub mod gateway;
mod json_size;
pub mod local;
pub mod policy;
mod schema;
pub mod transport;

pub use cache::{CacheMeta, CacheStore};
pub use events::ProxyEvent;
pub use gateway::{EventSink, McpGateway, ServerConfig};
pub use policy::{PolicySet, ToolPolicy};
pub use schema::ArtifactSchema;
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
    Local,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    pub content_for_model: serde_json::Value,
    pub artifacts: Vec<PathBuf>,
    pub provenance: Provenance,
}

/// Disposable correlation only. Never used to authorize or replay a call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CallContext {
    pub epoch: String,
    pub attempt: u64,
    pub call: u32,
}

pub trait ToolGateway: Send {
    /// Forward this through adapters so main and proxy observations can be joined.
    fn set_call_context(&mut self, _context: Option<CallContext>) {}
    /// Absolute cooperative deadline for the next dispatch, including pagination.
    fn set_deadline(&mut self, _deadline: Option<std::time::Instant>) {}
    fn list_tools(&self) -> Result<Vec<ToolDescriptor>>;
    /// Optional model guidance for exact `server.tool` grants. Must be
    /// nonblocking and network-free: return only locally available descriptors,
    /// skipping unavailable or locked catalogs. An empty result is valid;
    /// final dispatch must still validate credentials and the full wire schema.
    fn available_tools(&self, _grants: &[String]) -> Vec<ToolDescriptor> {
        Vec::new()
    }
    /// Existing run-local argument restrictions, for model guidance only.
    /// Dispatch must still validate both these restrictions and the wire schema.
    fn argument_constraints(&self, _server: &str, _tool: &str) -> Option<serde_json::Value> {
        None
    }
    /// Effective policy classification for this gateway. Must be stable for
    /// the run and must not dispatch tools. Adapters must forward it so core
    /// can enforce mutation journaling even when plan and gateway differ.
    fn tool_class(&self, server: &str, tool: &str) -> policy::ToolClass;
    /// Validate a granted call before recording mutation intent. This must not
    /// dispatch tools. Concrete gateways also protect their direct call path.
    fn validate_call(&self, call: &ToolCall) -> Result<()>;
    /// Executes a tool call, applying gateway policies (cache/paginate/serveFrom/
    /// shape/mutation). Implementations must be the ONLY path models have to tools.
    fn call(&mut self, call: &ToolCall) -> Result<ToolResult>;
    /// Begin a static producer attempt before model generation. Unsupported adapters
    /// must reject rather than leave an earlier complete destination usable.
    fn begin_result_artifact_attempt(
        &mut self,
        _server: &str,
        _tool: &str,
        _name: &str,
    ) -> Result<()> {
        anyhow::bail!("gateway does not support task resultArtifact");
    }
    /// A per-dispatch storage choice, never wire arguments or correlation context.
    fn call_with_result_artifact(
        &mut self,
        call: &ToolCall,
        name: Option<&str>,
    ) -> Result<ToolResult> {
        anyhow::ensure!(
            name.is_none(),
            "gateway does not support task resultArtifact"
        );
        self.call(call)
    }
}
