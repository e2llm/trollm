//! Error types. Runtime task/node failures carry a closed error class (§4.5).

use serde::{Deserialize, Serialize};

/// Closed enum of error classes (§4.5). Serialized kebab-case in the event log.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ErrorClass {
    McpUnreachable,
    McpError,
    ToolCallParse,
    VerificationFailed,
    ArtifactIo,
    BudgetExhausted,
    ModelError,
}

impl ErrorClass {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::McpUnreachable => "mcp-unreachable",
            Self::McpError => "mcp-error",
            Self::ToolCallParse => "tool-call-parse",
            Self::VerificationFailed => "verification-failed",
            Self::ArtifactIo => "artifact-io",
            Self::BudgetExhausted => "budget-exhausted",
            Self::ModelError => "model-error",
        }
    }
}

impl std::fmt::Display for ErrorClass {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A classified task/node failure, routed through the node's `onError` (§4.5).
#[derive(Debug, thiserror::Error)]
#[error("{class}: {detail}")]
pub struct TaskFailure {
    pub class: ErrorClass,
    pub detail: String,
}

impl TaskFailure {
    pub fn new(class: ErrorClass, detail: impl Into<String>) -> Self {
        Self {
            class,
            detail: detail.into(),
        }
    }
}

/// Descriptive plan/registry validation failure (§4.2: load-time where
/// statically checkable). Aggregates every problem found.
#[derive(Debug, thiserror::Error)]
#[error("invalid {what}:\n{}", .problems.iter().map(|p| format!("  - {p}")).collect::<Vec<_>>().join("\n"))]
pub struct ValidationError {
    pub what: String,
    pub problems: Vec<String>,
}

impl ValidationError {
    pub fn new(what: impl Into<String>, problems: Vec<String>) -> Self {
        Self {
            what: what.into(),
            problems,
        }
    }
}
