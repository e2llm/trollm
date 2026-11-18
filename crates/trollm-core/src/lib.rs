//! trollm-core: plan model, validator, executor (stack machine), event
//! log, artifact store, budgets — the deterministic "ledger" half of
//! Trollm (docs/ARCHITECTURE-ANDROID.md §3–§9). All routing is
//! authoring-time; models emit prose plus the §5.1 tool-call fence.

pub mod artifact_view;
pub mod artifacts;
pub mod error;
pub mod events;
pub mod exec;
pub mod fence;
mod json_size;
pub mod plan;
pub mod registry;
pub mod state;
pub mod template;
pub mod validate;

pub use error::{ErrorClass, TaskFailure, ValidationError};
pub use events::{EventLog, EventType};
pub use exec::{Executor, RunDir, RunStatus, SlotMap};
pub use plan::{ExecutionPlan, Node, NodeKind, Transition};
pub use registry::{TaskDef, TaskExecution, TaskRegistry};
pub use state::RunState;
