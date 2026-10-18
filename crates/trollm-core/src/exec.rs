//! The executor: a stack machine over task/foreach/end (§4.4) with coder-loop
//! task semantics (§5), mutation journaling (§6), budgets, and pause/resume /
//! crash recovery via the persisted RunState (§4.4).

use anyhow::{anyhow, Context};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use trollm_mcp::ToolGateway;
use trollm_models::{FinishReason, GenerateRequest, ModelSlot};
use uuid::Uuid;

use crate::artifacts;
use crate::error::{ErrorClass, TaskFailure};
use crate::events::{now_ms, DanglingIntent, EventLog, EventType};
use crate::plan::*;
use crate::registry::*;
use crate::state::*;
use crate::{fence, template, validate};

/// Hash the same JSON bytes as Value::to_string without allocating that string.
fn hash_json(value: &Value) -> serde_json::Result<String> {
    struct HashWriter(Sha256);
    impl std::io::Write for HashWriter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.update(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut writer = HashWriter(Sha256::new());
    serde_json::to_writer(&mut writer, value)?;
    Ok(hex(&writer.0.finalize()))
}

/// Slot name → model instance (§9 leaves supporting types to core).
#[derive(Debug, Default)]
pub struct SlotMap<M> {
    slots: BTreeMap<String, M>,
}

impl<M> SlotMap<M> {
    pub fn new() -> Self {
        Self {
            slots: BTreeMap::new(),
        }
    }

    pub fn with(mut self, name: impl Into<String>, model: M) -> Self {
        self.slots.insert(name.into(), model);
        self
    }

    pub fn insert(&mut self, name: impl Into<String>, model: M) {
        self.slots.insert(name.into(), model);
    }

    pub fn get_mut(&mut self, name: &str) -> Option<&mut M> {
        self.slots.get_mut(name)
    }

    pub fn contains(&self, name: &str) -> bool {
        self.slots.contains_key(name)
    }
}

impl<M> FromIterator<(String, M)> for SlotMap<M> {
    fn from_iter<I: IntoIterator<Item = (String, M)>>(iter: I) -> Self {
        Self {
            slots: iter.into_iter().collect(),
        }
    }
}

/// `runs/<uuid>/` directory handle (§8).
#[derive(Debug, Clone)]
pub struct RunDir {
    path: PathBuf,
    run_id: String,
}

impl RunDir {
    /// Wrap an existing or new run directory; the run id is the directory name
    /// (a fresh uuid is generated only if the name is not one).
    pub fn new(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let run_id = path
            .file_name()
            .and_then(|n| n.to_str())
            .filter(|s| Uuid::parse_str(s).is_ok())
            .map(str::to_string)
            .unwrap_or_else(|| Uuid::new_v4().to_string());
        Self { path, run_id }
    }

    /// Create a fresh `runs/<uuid>` under `base` (§8 layout).
    pub fn under(base: &Path) -> Self {
        Self::new(base.join("runs").join(Uuid::new_v4().to_string()))
    }

    pub fn create_dirs(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(self.artifacts_dir())
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn run_id(&self) -> &str {
        &self.run_id
    }
    pub fn events_path(&self) -> PathBuf {
        self.path.join("events.jsonl")
    }
    pub fn snapshot_path(&self) -> PathBuf {
        self.path.join("state.snapshot.json")
    }
    pub fn artifacts_dir(&self) -> PathBuf {
        self.path.join("artifacts")
    }
}

/// §9 pinned enum.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunStatus {
    Running,
    Paused { reason: String },
    Succeeded,
    Failed { reason: String },
}

/// Optional cooperative control at dispatch and retry boundaries. An active
/// model or tool call remains synchronous; the next boundary observes cancel.
pub trait ExecutionControl: Send + Sync {
    /// Hold for a user pause if needed, then return true if cancelled.
    fn checkpoint(&self) -> bool;
    /// Wait for retry backoff, preserving any pause and returning true on cancel.
    fn wait_backoff(&self, duration: Duration) -> bool;
}

/// Cloned identity of the plan owning the current frame (avoids holding a
/// borrow into `self.plan` across `&mut self` calls).
#[derive(Debug, Clone)]
struct FramePlan {
    id: String,
    limits: Limits,
    on_limit: RunLimitPolicy,
}

/// Result of one successful coder loop (§5).
struct TaskOutcome {
    prose: String,
    tool_calls_made: u32,
    /// Artifacts reported by the gateway during this task, keyed by logical
    /// name (filename stem — the wiring convention documented in artifacts.rs).
    artifacts: BTreeMap<String, PathBuf>,
}

/// A successful task loop can stop at a control boundary after all available
/// call results have been journaled, without inventing an authored error.
enum TaskLoopOutcome {
    Completed(TaskOutcome),
    Cancelled,
}

pub struct Executor<M: ModelSlot, G: ToolGateway> {
    plan: ExecutionPlan,
    registry: TaskRegistry,
    user_input: String,
    models: SlotMap<M>,
    gateway: G,
    run_dir: RunDir,
    log: EventLog,
    state: RunState,
    /// True when constructed over an existing run dir (resume/crash recovery).
    restored: bool,
    start_events_emitted: bool,
    pending_uncertain: Vec<DanglingIntent>,
    unsafe_completed_replay: bool,
    /// Retries consumed on the current node (per-node budget, resets on entry).
    node_retries: u32,
    pause_reason: String,
    fail_reason: String,
    cancel_requested: bool,
    control: Option<Arc<dyn ExecutionControl>>,
}

impl<M: ModelSlot, G: ToolGateway> Executor<M, G> {
    /// §9 pinned constructor. Validates plan + registry (§4.2). If `run_dir`
    /// already contains `state.snapshot.json`, this is a resume/crash-recovery
    /// construction: state and event-log position are restored and dangling
    /// mutation intents are collected for `onUncertain` handling (§6).
    pub fn new(
        plan: ExecutionPlan,
        registry: TaskRegistry,
        user_input: String,
        models: SlotMap<M>,
        gateway: G,
        run_dir: RunDir,
    ) -> anyhow::Result<Self> {
        validate::validate_plan(&plan).map_err(|e| anyhow!(e.to_string()))?;
        registry.validate_completion()?;
        validate::validate_with_registry(&plan, &registry).map_err(|e| anyhow!(e.to_string()))?;
        validate::validate_mutation_safety(&plan, &registry, |server, tool| {
            gateway.tool_class(server, tool)
        })
        .map_err(|e| anyhow!(e.to_string()))?;
        fn check_completion_outputs(
            plan: &ExecutionPlan,
            registry: &TaskRegistry,
        ) -> anyhow::Result<()> {
            for (name, node) in &plan.nodes {
                if node
                    .task_ref
                    .as_deref()
                    .and_then(|task| registry.task(task))
                    .is_some_and(|task| task.completion == TaskCompletion::ToolResult)
                {
                    anyhow::ensure!(
                        !matches!(&node.outputs, Some(NodeOutputs::Task(outputs)) if outputs.values().any(|output| matches!(output, NodeOutputBinding::FromModel { .. }))),
                        "plan '{}' node '{name}': tool-result completion cannot bind model outputs",
                        plan.id
                    );
                }
            }
            for child in plan.subflows.values() {
                check_completion_outputs(child, registry)?;
            }
            Ok(())
        }
        check_completion_outputs(&plan, &registry)?;
        // Every slot named by any plan in the tree must be bound.
        fn check_slots(
            plan: &ExecutionPlan,
            models: &SlotMap<impl Sized>,
            problems: &mut Vec<String>,
        ) {
            for slot in plan.models.keys() {
                if !models.contains(slot) {
                    problems.push(format!(
                        "plan '{}' declares slot '{slot}' but no model is bound to it",
                        plan.id
                    ));
                }
            }
            for sub in plan.subflows.values() {
                check_slots(sub, models, problems);
            }
        }
        let mut problems = Vec::new();
        check_slots(&plan, &models, &mut problems);
        if !problems.is_empty() {
            return Err(anyhow!(problems.join("\n")));
        }

        run_dir.create_dirs().context("creating run dir")?;
        let log = EventLog::open(&run_dir.events_path(), run_dir.run_id())
            .context("opening event log")?;

        let snapshot = run_dir.snapshot_path();
        let (state, restored, pending_uncertain, unsafe_completed_replay) = if snapshot.exists() {
            let bytes = std::fs::read(&snapshot).context("reading state.snapshot.json")?;
            let mut state: RunState =
                serde_json::from_slice(&bytes).context("parsing state.snapshot.json")?;
            // The maxItems window is per dispatch session: after pause/resume a
            // run may process up to maxItems further items (PREMISE: pause
            // composes with backgrounding; resume-from-cursor is the expected
            // path). Documented deviation-free reading of §3/§4.4.
            for cur in state.foreach_cursors.values_mut() {
                cur.session_items = 0;
            }
            let mut pending = BTreeMap::new();
            let mut answered = std::collections::BTreeSet::new();
            let mut unsafe_replay = false;
            EventLog::visit(
                &run_dir.events_path(),
                Some(run_dir.run_id()),
                |event, _| match event["type"].as_str() {
                    Some("tool_call_intent") => {
                        let seq = event["seq"].as_u64().expect("validated event sequence");
                        if !answered.contains(&seq) {
                            pending.insert(seq, DanglingIntent::from_event(seq, event));
                        }
                    }
                    Some("tool_call_result") => {
                        if let Some(intent) = event["intentSeq"].as_u64() {
                            answered.insert(intent);
                            pending.remove(&intent);
                            if event["seq"].as_u64().unwrap_or(u64::MAX) > state.event_watermark {
                                unsafe_replay = true;
                            }
                        }
                    }
                    _ => {}
                },
            )?;
            let pending = pending.into_values().collect();
            (state, true, pending, unsafe_replay)
        } else {
            (
                RunState::fresh(run_dir.run_id(), &plan.pin(), &plan.entry_step),
                false,
                Vec::new(),
                false,
            )
        };

        let fail_reason = state
            .terminal_reason
            .clone()
            .unwrap_or_else(|| "run already failed".to_string());
        Ok(Self {
            plan,
            registry,
            user_input,
            models,
            gateway,
            run_dir,
            log,
            state,
            restored,
            start_events_emitted: false,
            pending_uncertain,
            unsafe_completed_replay,
            node_retries: 0,
            pause_reason: String::new(),
            fail_reason,
            cancel_requested: false,
            control: None,
        })
    }

    /// Install the run's cooperative control before driving this executor.
    pub fn set_control(&mut self, control: Arc<dyn ExecutionControl>) {
        self.control = Some(control);
    }

    pub fn run_dir(&self) -> &RunDir {
        &self.run_dir
    }

    pub fn run_id(&self) -> &str {
        self.run_dir.run_id()
    }

    pub fn state(&self) -> &RunState {
        &self.state
    }

    /// §9: drive until Paused / Succeeded / Failed.
    pub fn run_until_blocked(&mut self) -> anyhow::Result<RunStatus> {
        if !self.start_events_emitted {
            self.start_events_emitted = true;
            if !self.restored {
                self.emit(
                    EventType::RunStarted,
                    json!({ "plan": self.plan.id, "sha256": self.plan.sha256 }),
                )?;
                let mut pins = Vec::new();
                collect_pins(&self.plan, &mut pins);
                for (id, sha) in pins {
                    self.emit(EventType::PlanLoaded, json!({ "plan": id, "sha256": sha }))?;
                }
                let (pid, node) = self.current_position()?;
                self.emit(EventType::NodeEntered, json!({ "plan": pid, "node": node }))?;
                self.persist_snapshot()?;
            }
        }
        if self.restored {
            self.restored = false;
            match self.state.status {
                StateStatus::Succeeded => return Ok(RunStatus::Succeeded),
                StateStatus::Failed => {
                    return Ok(RunStatus::Failed {
                        reason: self.fail_reason.clone(),
                    })
                }
                _ => {}
            }
            self.state.status = StateStatus::Running;
            self.emit(EventType::Resumed, json!({}))?;
            self.handle_recovery()?;
            if self.state.status != StateStatus::Running {
                self.persist_snapshot()?;
                return self.current_status();
            }
            self.persist_snapshot()?;
        }
        self.dispatch()
    }

    /// §9: resume a paused run (same process or after crash — identical path).
    pub fn resume(&mut self) -> anyhow::Result<RunStatus> {
        match self.state.status {
            StateStatus::Succeeded => return Ok(RunStatus::Succeeded),
            StateStatus::Failed => {
                return Ok(RunStatus::Failed {
                    reason: self.fail_reason.clone(),
                })
            }
            _ => {}
        }
        if self.restored {
            self.restored = false;
        }
        self.state.status = StateStatus::Running;
        // maxItems is a per-session window; resuming starts a new session.
        for cur in self.state.foreach_cursors.values_mut() {
            cur.session_items = 0;
        }
        self.emit(EventType::Resumed, json!({}))?;
        self.handle_recovery()?;
        if self.state.status != StateStatus::Running {
            self.persist_snapshot()?;
            return self.current_status();
        }
        self.persist_snapshot()?;
        self.dispatch()
    }

    /// §9: cancel. Terminal; recorded in the log.
    pub fn cancel(&mut self) {
        let _ = self.try_cancel();
    }

    /// Fallible cancellation for callers that must report durable outcome.
    pub fn try_cancel(&mut self) -> anyhow::Result<()> {
        self.cancel_requested = true;
        if matches!(
            self.state.status,
            StateStatus::Running | StateStatus::Paused
        ) {
            self.finish_cancelled()?;
        }
        Ok(())
    }

    /// §9: persist RunState without committing additional log progress.
    /// The executor advances the recovery watermark only at node boundaries.
    pub fn snapshot(&self) -> anyhow::Result<()> {
        self.write_snapshot()
    }

    // ------------------------------------------------------------------
    // dispatch

    fn dispatch(&mut self) -> anyhow::Result<RunStatus> {
        loop {
            match self.state.status {
                StateStatus::Paused => {
                    return Ok(RunStatus::Paused {
                        reason: self.pause_reason.clone(),
                    })
                }
                StateStatus::Succeeded => return Ok(RunStatus::Succeeded),
                StateStatus::Failed => {
                    return Ok(RunStatus::Failed {
                        reason: self.fail_reason.clone(),
                    })
                }
                StateStatus::Running => {}
            }
            if self.control_cancelled() {
                self.finish_cancelled()?;
                continue;
            }

            let (plan_id, node_id) = self.current_position()?;
            let fp = self.frame_plan(&plan_id)?;
            let node = fp_node(&self.plan, &plan_id, &node_id)?.clone();
            match node.kind {
                NodeKind::Task => self.step_task(&node_id, &node, &fp)?,
                NodeKind::Foreach => self.step_foreach(&node_id, &node, &fp)?,
                NodeKind::End => self.step_end(&node_id, &node, &fp)?,
            }
        }
    }

    fn current_status(&self) -> anyhow::Result<RunStatus> {
        Ok(match self.state.status {
            StateStatus::Running => RunStatus::Running,
            StateStatus::Paused => RunStatus::Paused {
                reason: self.pause_reason.clone(),
            },
            StateStatus::Succeeded => RunStatus::Succeeded,
            StateStatus::Failed => RunStatus::Failed {
                reason: self.fail_reason.clone(),
            },
        })
    }

    fn current_position(&self) -> anyhow::Result<(String, String)> {
        let frame = self
            .state
            .stack
            .last()
            .ok_or_else(|| anyhow!("run stack is empty"))?;
        Ok((plan_id_of_pin(&frame.plan), frame.node.clone()))
    }

    fn frame_plan(&self, plan_id: &str) -> anyhow::Result<FramePlan> {
        let plan = self
            .plan
            .find_plan(plan_id)
            .ok_or_else(|| anyhow!("plan '{plan_id}' referenced by run state is not loaded"))?;
        Ok(FramePlan {
            id: plan.id.clone(),
            limits: plan.limits.clone(),
            on_limit: plan.on_limit,
        })
    }

    fn emit(&mut self, ty: EventType, fields: Value) -> std::io::Result<u64> {
        self.log.append(ty, fields)
    }

    fn persist_snapshot(&mut self) -> anyhow::Result<()> {
        self.state.event_watermark = self.log.last_seq();
        self.write_snapshot()
    }

    fn write_snapshot(&self) -> anyhow::Result<()> {
        let bytes = serde_json::to_vec_pretty(&self.state)?;
        let path = self.run_dir.snapshot_path();
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, bytes)?;
        std::fs::rename(&tmp, &path)?;
        Ok(())
    }

    fn finish_failed(&mut self, reason: &str) -> anyhow::Result<()> {
        self.emit(
            EventType::RunEnded,
            json!({ "outcome": "failed", "reason": reason }),
        )?;
        self.state.status = StateStatus::Failed;
        self.fail_reason = reason.to_string();
        self.state.terminal_reason = Some(reason.to_string());
        self.persist_snapshot()
    }

    fn finish_cancelled(&mut self) -> anyhow::Result<()> {
        self.emit(EventType::RunEnded, json!({ "outcome": "cancelled" }))?;
        self.state.status = StateStatus::Failed;
        self.fail_reason = "cancelled".to_string();
        self.state.terminal_reason = Some(self.fail_reason.clone());
        self.persist_snapshot()
    }

    fn control_cancelled(&self) -> bool {
        self.cancel_requested
            || self
                .control
                .as_ref()
                .is_some_and(|control| control.checkpoint())
    }

    fn finish_succeeded(&mut self, outcome: &str) -> anyhow::Result<()> {
        self.emit(EventType::RunEnded, json!({ "outcome": outcome }))?;
        self.state.status = StateStatus::Succeeded;
        self.persist_snapshot()
    }

    /// Move the top frame along a transition (§4.2): node target, or inline
    /// `{status: paused|failed}`.
    fn transit(&mut self, key: &str, t: &Transition) -> anyhow::Result<()> {
        let (plan_id, from) = self.current_position()?;
        match t {
            Transition::Node(target) => {
                self.emit(
                    EventType::NodeExited,
                    json!({ "plan": plan_id, "node": from }),
                )?;
                self.emit(
                    EventType::Transition,
                    json!({ "plan": plan_id, "from": from, "to": target, "key": key }),
                )?;
                let frame = self.state.stack.last_mut().unwrap();
                frame.node = target.clone();
                self.node_retries = 0;
                self.emit(
                    EventType::NodeEntered,
                    json!({ "plan": plan_id, "node": target }),
                )?;
                self.persist_snapshot()?;
            }
            Transition::Inline(inline) => {
                let to = match inline.status {
                    InlineStatus::Paused => "paused",
                    InlineStatus::Failed => "failed",
                };
                self.emit(
                    EventType::Transition,
                    json!({ "plan": plan_id, "from": from, "to": to, "key": key }),
                )?;
                match inline.status {
                    InlineStatus::Paused => {
                        self.emit(EventType::Paused, json!({ "reason": inline.reason }))?;
                        self.state.status = StateStatus::Paused;
                        self.pause_reason = inline.reason.clone();
                        self.persist_snapshot()?;
                    }
                    InlineStatus::Failed => {
                        let reason = if inline.reason.is_empty() {
                            format!("transition '{key}' from '{from}' declared failed")
                        } else {
                            inline.reason.clone()
                        };
                        self.finish_failed(&reason)?;
                    }
                }
            }
        }
        Ok(())
    }

    // ------------------------------------------------------------------
    // end nodes

    fn step_end(&mut self, node_id: &str, node: &Node, fp: &FramePlan) -> anyhow::Result<()> {
        let outcome = node.outcome.clone().unwrap_or_default();
        self.emit(
            EventType::NodeExited,
            json!({ "plan": fp.id, "node": node_id, "outcome": outcome }),
        )?;
        if self.state.stack.len() == 1 {
            // Root end. The frame is kept on the stack so the terminal
            // snapshot retains the run's bound outputs (consumers read final
            // results from state.snapshot.json without re-parsing the log).
            // Only succeeded/failed are routable root outcomes; anything else
            // (e.g. a subflow-only outcome like 'skipped' surfacing at root)
            // is the runtime unhandled-transition case of §4.2.
            match outcome.as_str() {
                "succeeded" => self.finish_succeeded("succeeded")?,
                "failed" => self.finish_failed("failed")?,
                other => {
                    self.finish_failed(&format!(
                        "unhandled-transition: root end outcome '{other}'"
                    ))?;
                }
            }
        } else {
            // flowRef return (§4.4): hand the child frame's node outputs to the
            // parent foreach for §4.6 aggregation.
            let frame = self.state.stack.pop().unwrap();
            let parent = self.state.stack.last().unwrap();
            self.state.pending_return = Some(PendingReturn {
                parent_plan: parent.plan.clone(),
                parent_node: parent.node.clone(),
                outputs: frame.node_outputs,
                outcome,
            });
            self.persist_snapshot()?;
        }
        Ok(())
    }

    // ------------------------------------------------------------------
    // task nodes (§5)

    fn step_task(&mut self, node_id: &str, node: &Node, fp: &FramePlan) -> anyhow::Result<()> {
        // Timeout is frame-scoped and checked on task entry only. A final long
        // task may exceed it and still succeed; inference is not preempted.
        let started = self
            .state
            .stack
            .last()
            .map(|f| f.counters.started_at)
            .unwrap_or_else(now_ms);
        if now_ms().saturating_sub(started) > fp.limits.timeout_s * 1000 {
            self.emit(
                EventType::BudgetExhausted,
                json!({ "budget": "timeoutS", "limit": fp.limits.timeout_s, "node": node_id }),
            )?;
            return self.apply_run_limit(fp, "timeoutS");
        }

        let task = self
            .registry
            .task(node.task_ref.as_deref().unwrap_or(""))
            .cloned()
            .ok_or_else(|| anyhow!("taskRef {:?} not in registry", node.task_ref))?;

        let outcome = self.run_task_loop(node_id, node, &task, fp);
        let cancelled = self.control_cancelled();
        match outcome {
            // A failed journal write must never be reported as a durable
            // cancellation, even if the user cancels while that write fails.
            Err(f) if cancelled && f.class == ErrorClass::ArtifactIo => Err(anyhow!(f)),
            Ok(TaskLoopOutcome::Cancelled) => self.finish_cancelled(),
            _ if cancelled => self.finish_cancelled(),
            Ok(TaskLoopOutcome::Completed(outcome)) => {
                match self.verify_and_bind(node_id, node, &task, &outcome) {
                    Ok(()) => {
                        if self.control_cancelled() {
                            return self.finish_cancelled();
                        }
                        let t = node.on_verified.clone().expect("validated");
                        self.transit("onVerified", &t)
                    }
                    Err(f) => self.handle_task_error(node_id, node, fp, f),
                }
            }
            Err(f) if f.class == ErrorClass::BudgetExhausted => self.apply_run_limit(fp, &f.detail),
            Err(f) => self.handle_task_error(node_id, node, fp, f),
        }
    }

    /// Coder loop (§5): render → generate → fences? gateway executes, append
    /// result, repeat : the text is the node's prose output → verify →
    /// transition. Loop exit without prose after maxSteps = verification-failed.
    /// Opt-in tool-result completion returns after one successful tool call;
    /// output/artifact verification still runs before transition.
    fn run_task_loop(
        &mut self,
        node_id: &str,
        node: &Node,
        task: &TaskDef,
        fp: &FramePlan,
    ) -> Result<TaskLoopOutcome, TaskFailure> {
        let task_ref = node.task_ref.clone().unwrap_or_default();
        let frame_vars = self
            .state
            .stack
            .last()
            .map(|f| f.vars.clone())
            .unwrap_or(Value::Null);
        let mut bound_arguments = BTreeMap::new();
        if let Some(bindings) = &task.tool_arguments {
            let input = if bindings.values().any(|rule| !rule.from_input.is_empty()) {
                match serde_json::from_str::<Value>(&self.user_input) {
                    Ok(Value::Object(input)) => Some(input),
                    _ => {
                        return Err(TaskFailure::new(
                            ErrorClass::VerificationFailed,
                            "fromInput requires user_input to be a JSON object",
                        ))
                    }
                }
            } else {
                None
            };
            for (tool, rule) in bindings {
                let mut values = BTreeMap::new();
                for (argument, path) in &rule.from_vars {
                    let value = template::lookup(path, &frame_vars).ok_or_else(|| {
                        TaskFailure::new(
                            ErrorClass::VerificationFailed,
                            format!("tool argument source '{path}' is not bound"),
                        )
                    })?;
                    values.insert(argument.clone(), (value.clone(), "fromVars"));
                }
                for (argument, key) in &rule.from_input {
                    let value =
                        input
                            .as_ref()
                            .and_then(|input| input.get(key))
                            .ok_or_else(|| {
                                TaskFailure::new(
                                    ErrorClass::VerificationFailed,
                                    format!("fromInput source '{key}' is not bound"),
                                )
                            })?;
                    values.insert(argument.clone(), (value.clone(), "fromInput"));
                }
                for (argument, value) in &rule.fixed {
                    values.insert(argument.clone(), (value.clone(), "fixed"));
                }
                bound_arguments.insert(tool.clone(), values);
            }
        }
        let template_text = self
            .registry
            .prompt_template(task)
            .map_err(|e| TaskFailure::new(ErrorClass::ArtifactIo, format!("prompt: {e}")))?;
        let mut context =
            template::render(&template_text, &frame_vars, &self.user_input).map_err(|e| {
                TaskFailure::new(
                    ErrorClass::VerificationFailed,
                    format!("prompt render: {e}"),
                )
            })?;

        let max_steps = task.loop_spec.as_ref().map(|l| l.max_steps).unwrap_or(1);
        let mut tool_calls_made = 0u32;
        let mut artifacts: BTreeMap<String, PathBuf> = BTreeMap::new();
        let mut prose: Option<String> = None;

        for _ in 0..max_steps {
            self.check_generation_budget(node_id, task, fp)?;

            let req = GenerateRequest {
                prompt: context.clone(),
                grammar_gbnf: if task.tools.is_empty() {
                    None
                } else {
                    Some(trollm_models::fence::tool_call_grammar(
                        &task
                            .tools
                            .iter()
                            .filter_map(|grant| grant.split_once('.'))
                            .map(|(server, tool)| (server.to_string(), tool.to_string()))
                            .collect::<Vec<_>>(),
                    ))
                },
                max_tokens: 2048,
                stop: Vec::new(),
            };
            let t0 = Instant::now();
            let resp = {
                let slot = self.models.get_mut(&task.model).ok_or_else(|| {
                    TaskFailure::new(
                        ErrorClass::ModelError,
                        format!("no model bound to slot '{}'", task.model),
                    )
                })?;
                slot.generate(req)
                    .map_err(|e| TaskFailure::new(ErrorClass::ModelError, e.to_string()))?
            };
            let ms = t0.elapsed().as_millis() as u64;

            {
                let frame = self.state.stack.last_mut().unwrap();
                frame.counters.steps += 1;
                frame.counters.tokens += resp.tokens_in as u64 + resp.tokens_out as u64;
                if task.model == CANONICAL_REASONER_SLOT {
                    frame.counters.reasoner_calls += 1;
                }
            }
            self.state.counters.steps += 1;
            self.state.counters.tokens += resp.tokens_in as u64 + resp.tokens_out as u64;
            if task.model == CANONICAL_REASONER_SLOT {
                self.state.counters.reasoner_calls += 1;
            }
            self.emit(
                EventType::ModelCall,
                json!({
                    "slot": task.model, "node": node_id,
                    "tokensIn": resp.tokens_in, "tokensOut": resp.tokens_out, "ms": ms,
                    "finishReason": match resp.finish {
                        FinishReason::Stop => "stop",
                        FinishReason::Length => "length",
                        FinishReason::GrammarReject => "grammar-reject",
                    },
                }),
            )
            .map_err(|e| TaskFailure::new(ErrorClass::ArtifactIo, format!("event log: {e}")))?;

            if self.control_cancelled() {
                return Ok(TaskLoopOutcome::Cancelled);
            }

            if let Some(frame) = self.state.stack.last() {
                if frame.counters.tokens > fp.limits.max_tokens_total {
                    self.emit(
                        EventType::BudgetExhausted,
                        json!({ "budget": "maxTokensTotal", "value": frame.counters.tokens,
                                "limit": fp.limits.max_tokens_total, "node": node_id }),
                    )
                    .map_err(|e| TaskFailure::new(ErrorClass::ArtifactIo, e.to_string()))?;
                    return Err(TaskFailure::new(
                        ErrorClass::BudgetExhausted,
                        format!("maxTokensTotal {} exceeded", fp.limits.max_tokens_total),
                    ));
                }
            }

            match resp.finish {
                FinishReason::Stop => {}
                FinishReason::Length => {
                    return Err(TaskFailure::new(
                        ErrorClass::ModelError,
                        "model capacity exhausted: generation reached its token limit (Length)",
                    ));
                }
                FinishReason::GrammarReject => {
                    return Err(TaskFailure::new(
                        ErrorClass::ModelError,
                        "model grammar rejected generation (GrammarReject)",
                    ));
                }
            }

            let mut calls = fence::extract_tool_calls(&resp.text)
                .map_err(|e| TaskFailure::new(ErrorClass::ToolCallParse, e.to_string()))?;
            if task.completion == TaskCompletion::ToolResult && calls.len() != 1 {
                return Err(TaskFailure::new(
                    ErrorClass::ToolCallParse,
                    "tool-result completion requires exactly one tool call",
                ));
            }
            if calls.is_empty() {
                // No fence: this generation is the node's prose output (§5).
                prose = Some(resp.text.trim().to_string());
                break;
            }

            for call in &mut calls {
                let fqn = format!("{}.{}", call.server, call.tool);
                if !task.tools.iter().any(|t| t == &fqn) {
                    // Post-hoc validation failure (§5.1): the model called a
                    // tool outside its allowlist. Counted as tool-call-parse
                    // (see OPEN QUESTIONS in the implementation report).
                    return Err(TaskFailure::new(
                        ErrorClass::ToolCallParse,
                        format!("tool '{fqn}' not in allowlist of task '{task_ref}'"),
                    ));
                }

                if let Some(bindings) = bound_arguments.get(&fqn) {
                    let args = call.args.as_object_mut().ok_or_else(|| {
                        TaskFailure::new(ErrorClass::ToolCallParse, "tool args must be an object")
                    })?;
                    for (name, (value, source)) in bindings {
                        match args.get(name) {
                            Some(provided) if provided != value => {
                                return Err(TaskFailure::new(
                                    ErrorClass::ToolCallParse,
                                    format!(
                                        "tool '{fqn}' argument '{name}' conflicts with {source}"
                                    ),
                                ));
                            }
                            Some(_) => {}
                            None => {
                                args.insert(name.clone(), value.clone());
                            }
                        }
                    }
                }

                // Recheck the actual dispatch policy; plan/gateway composition
                // must never allow an unjournaled declared mutation.
                let journaled = task
                    .mutations
                    .as_ref()
                    .is_some_and(|m| m.idempotency == Idempotency::IntentLog);
                if (self.gateway.tool_class(&call.server, &call.tool)
                    == trollm_mcp::policy::ToolClass::Mutation
                    || self
                        .plan
                        .mcp
                        .policies
                        .get(&fqn)
                        .and_then(|p| p.get("class"))
                        .and_then(|c| c.as_str())
                        == Some("mutation"))
                    && !journaled
                {
                    return Err(TaskFailure::new(
                        ErrorClass::McpError,
                        format!("mutation '{fqn}' requires intent-log before dispatch"),
                    ));
                }
                self.gateway.validate_call(call).map_err(|e| {
                    TaskFailure::new(ErrorClass::ToolCallParse, format!("{fqn}: {e:#}"))
                })?;

                // Mutation journaling (§6): intent BEFORE the call for tasks
                // with mutations.idempotency = intent-log.
                let intent_seq = if task
                    .mutations
                    .as_ref()
                    .map(|m| m.idempotency == Idempotency::IntentLog)
                    .unwrap_or(false)
                {
                    let args_sha256 = hash_json(&call.args)
                        .map_err(|e| TaskFailure::new(ErrorClass::ArtifactIo, e.to_string()))?;
                    Some(
                        self.emit(
                            EventType::ToolCallIntent,
                            json!({
                                "node": node_id, "taskRef": task_ref,
                                "server": call.server, "tool": call.tool,
                                "args": call.args, "argsSha256": args_sha256,
                            }),
                        )
                        .map_err(|e| TaskFailure::new(ErrorClass::ArtifactIo, e.to_string()))?,
                    )
                } else {
                    None
                };

                let t0 = Instant::now();
                let result = self
                    .gateway
                    .call(call)
                    .map_err(|e| TaskFailure::new(ErrorClass::McpError, format!("{fqn}: {e}")))?;
                let call_ms = t0.elapsed().as_millis() as u64;

                self.emit(
                    EventType::ToolCallResult,
                    json!({
                        "intentSeq": intent_seq, "node": node_id,
                        "server": call.server, "tool": call.tool,
                        "provenance": format!("{:?}", result.provenance), "ms": call_ms,
                    }),
                )
                .map_err(|e| TaskFailure::new(ErrorClass::ArtifactIo, e.to_string()))?;
                tool_calls_made += 1;

                for art in &result.artifacts {
                    if let Ok(bytes) = std::fs::read(art) {
                        let sha = hex_sha256(&bytes);
                        self.emit(
                            EventType::ArtifactWritten,
                            json!({ "path": art, "bytes": bytes.len(), "sha256": sha }),
                        )
                        .map_err(|e| TaskFailure::new(ErrorClass::ArtifactIo, e.to_string()))?;
                        self.account_artifact_bytes(bytes.len() as u64, fp)?;
                        if let Some(name) = artifacts::logical_name(art) {
                            artifacts.insert(name, art.clone());
                        }
                    }
                }

                // The result, including a mutation's intent linkage, is
                // durable before honoring a cancellation during the call.
                // This typed outcome bypasses authored onError.
                if self.control_cancelled() {
                    return Ok(TaskLoopOutcome::Cancelled);
                }

                if task.completion == TaskCompletion::ToolResult {
                    return Ok(TaskLoopOutcome::Completed(TaskOutcome {
                        // This mode forbids all model-prose output bindings.
                        prose: String::new(),
                        tool_calls_made,
                        artifacts,
                    }));
                }

                // Append result, repeat (§5).
                context.push_str("\n<tool_result>");
                context.push_str(
                    &json!({
                        "server": call.server,
                        "tool": call.tool,
                        "result": result.content_for_model,
                    })
                    .to_string(),
                );
                context.push_str("</tool_result>\n");
            }
        }

        let prose = prose.ok_or_else(|| {
            TaskFailure::new(
                ErrorClass::VerificationFailed,
                format!("coder loop exhausted maxSteps={max_steps} without prose output"),
            )
        })?;
        Ok(TaskLoopOutcome::Completed(TaskOutcome {
            prose,
            tool_calls_made,
            artifacts,
        }))
    }

    /// Charge every written artifact to its owning frame and the run mirror.
    /// The aggregate is diagnostic; a child's allowance excludes other frames.
    fn account_artifact_bytes(&mut self, bytes: u64, fp: &FramePlan) -> Result<(), TaskFailure> {
        self.state.counters.artifact_bytes += bytes;
        let frame = self.state.stack.last_mut().expect("active frame");
        frame.counters.artifact_bytes += bytes;
        let value = frame.counters.artifact_bytes;
        let limit = fp.limits.artifact_max_mb.saturating_mul(1024 * 1024);
        if value > limit {
            self.emit(
                EventType::BudgetExhausted,
                json!({ "budget": "artifactMaxMb", "value": value, "limit": limit }),
            )
            .map_err(|e| TaskFailure::new(ErrorClass::ArtifactIo, e.to_string()))?;
            return Err(TaskFailure::new(
                ErrorClass::BudgetExhausted,
                "artifactMaxMb exceeded",
            ));
        }
        Ok(())
    }

    /// Frame-scoped run budgets (§4.4: "own counter scope"; counters persist
    /// across pause, never reset). Checked before each generation.
    fn check_generation_budget(
        &mut self,
        node_id: &str,
        task: &TaskDef,
        fp: &FramePlan,
    ) -> Result<(), TaskFailure> {
        let (steps, reasoner_calls) = self
            .state
            .stack
            .last()
            .map(|f| (f.counters.steps, f.counters.reasoner_calls))
            .unwrap_or((0, 0));
        let hit = if steps >= fp.limits.max_steps {
            Some(("maxSteps", steps as u64, fp.limits.max_steps as u64))
        } else if task.model == CANONICAL_REASONER_SLOT
            && reasoner_calls >= fp.limits.max_reasoner_calls
        {
            Some((
                "maxReasonerCalls",
                reasoner_calls as u64,
                fp.limits.max_reasoner_calls as u64,
            ))
        } else {
            None
        };
        if let Some((budget, value, limit)) = hit {
            self.emit(
                EventType::BudgetExhausted,
                json!({ "budget": budget, "value": value, "limit": limit, "node": node_id }),
            )
            .map_err(|e| TaskFailure::new(ErrorClass::ArtifactIo, e.to_string()))?;
            return Err(TaskFailure::new(
                ErrorClass::BudgetExhausted,
                format!("{budget} {value}/{limit}"),
            ));
        }
        Ok(())
    }

    /// Run-level onLimit policy (§3): abort | respond_partial.
    fn apply_run_limit(&mut self, fp: &FramePlan, detail: &str) -> anyhow::Result<()> {
        match fp.on_limit {
            RunLimitPolicy::Abort => self.finish_failed(&format!("budget-exhausted: {detail}")),
            RunLimitPolicy::RespondPartial => {
                // Terminal but not a failure: the run ends and whatever partial
                // results exist are delivered.
                self.finish_succeeded("partial")
            }
        }
    }

    /// onError (§4.5): retry with backoff, then route to `then`. Default:
    /// retry 0, then `{status: failed, reason: <error-class>}`.
    fn handle_task_error(
        &mut self,
        node_id: &str,
        node: &Node,
        fp: &FramePlan,
        failure: TaskFailure,
    ) -> anyhow::Result<()> {
        if self.control_cancelled() {
            if failure.class == ErrorClass::ArtifactIo {
                return Err(anyhow!(failure));
            }
            return self.finish_cancelled();
        }
        self.emit(
            EventType::Error,
            json!({
                "class": failure.class.as_str(),
                "detail": failure.detail,
                "node": node_id,
                "plan": fp.id,
            }),
        )?;
        let policy = node.on_error.clone().unwrap_or(OnError {
            retry: 0,
            backoff_s: 0,
            then: Transition::Inline(InlineTransition {
                status: InlineStatus::Failed,
                reason: failure.class.as_str().to_string(),
            }),
        });
        if self.node_retries < policy.retry {
            self.node_retries += 1;
            if self.control_cancelled() {
                return self.finish_cancelled();
            }
            if policy.backoff_s > 0 {
                let delay = Duration::from_secs(policy.backoff_s);
                let cancelled = if let Some(control) = &self.control {
                    control.wait_backoff(delay)
                } else {
                    let mut remaining = delay;
                    while !remaining.is_zero() {
                        let chunk = remaining.min(Duration::from_secs(60));
                        std::thread::sleep(chunk);
                        remaining -= chunk;
                    }
                    false
                };
                if cancelled {
                    return self.finish_cancelled();
                }
            }
            if self.control_cancelled() {
                return self.finish_cancelled();
            }
            // Frame stays on this node: the task re-executes from entry
            // (per-node loop budget resets, §4.4).
            Ok(())
        } else {
            self.node_retries = 0;
            self.transit("onError", &policy.then)
        }
    }

    /// Output-contract verification (§4.3): structure only — required outputs
    /// exist, types match, artifact references exist and are complete.
    fn verify_and_bind(
        &mut self,
        node_id: &str,
        node: &Node,
        task: &TaskDef,
        outcome: &TaskOutcome,
    ) -> Result<(), TaskFailure> {
        let contract = &task.output_contract;

        // allowNoop (§4.3): a task with a tool allowlist that made no calls is
        // a no-op; the contract must explicitly accept that.
        if !contract.allow_noop && !task.tools.is_empty() && outcome.tool_calls_made == 0 {
            return Err(TaskFailure::new(
                ErrorClass::VerificationFailed,
                format!(
                    "task '{}' made no tool calls (no-op) and its contract does not allowNoop",
                    node.task_ref.as_deref().unwrap_or("")
                ),
            ));
        }

        let mut bound: BTreeMap<String, Value> = BTreeMap::new();
        for (name, spec) in &contract.outputs {
            if matches!(&node.outputs, Some(NodeOutputs::Task(outputs)) if outputs.contains_key(name))
            {
                continue;
            }
            let value = match spec {
                OutputSpec::FromModel { type_, .. } => {
                    let v = Value::String(outcome.prose.clone());
                    check_output_type(name, &v, *type_)?;
                    v
                }
                OutputSpec::FromArtifact {
                    from_artifact,
                    format,
                    type_,
                } => {
                    let path = self.find_artifact(from_artifact, &outcome.artifacts)?;
                    // §4.3: fromArtifact outputs require the meta sidecar to
                    // say complete: true.
                    if !artifacts::is_complete(&path) {
                        return Err(TaskFailure::new(
                            ErrorClass::VerificationFailed,
                            format!("artifact '{from_artifact}' is not marked complete"),
                        ));
                    }
                    let bytes = std::fs::read(&path).map_err(|e| {
                        TaskFailure::new(
                            ErrorClass::ArtifactIo,
                            format!("reading artifact '{from_artifact}': {e}"),
                        )
                    })?;
                    let v = parse_artifact(format, &bytes).map_err(|e| {
                        TaskFailure::new(
                            ErrorClass::ArtifactIo,
                            format!("parsing artifact '{from_artifact}': {e}"),
                        )
                    })?;
                    check_output_type(name, &v, *type_)?;
                    v
                }
            };
            bound.insert(name.clone(), value);
        }

        // Plan-node-level output bindings (§3) add to / override contract
        // bindings when present.
        if let Some(NodeOutputs::Task(outputs)) = &node.outputs {
            for (name, binding) in outputs {
                match binding {
                    NodeOutputBinding::FromModel { .. } => {
                        bound.insert(name.clone(), Value::String(outcome.prose.clone()));
                    }
                    NodeOutputBinding::FromArtifact {
                        from_artifact,
                        format,
                    } => {
                        let path = self.find_artifact(from_artifact, &outcome.artifacts)?;
                        if !artifacts::is_complete(&path) {
                            return Err(TaskFailure::new(
                                ErrorClass::VerificationFailed,
                                format!("artifact '{from_artifact}' is not marked complete"),
                            ));
                        }
                        let bytes = std::fs::read(&path)
                            .map_err(|e| TaskFailure::new(ErrorClass::ArtifactIo, e.to_string()))?;
                        let fmt = match format.as_deref() {
                            Some("json") => ArtifactFormat::Json,
                            _ => ArtifactFormat::Jsonl,
                        };
                        let v = parse_artifact(&fmt, &bytes)
                            .map_err(|e| TaskFailure::new(ErrorClass::ArtifactIo, e.to_string()))?;
                        bound.insert(name.clone(), v);
                    }
                }
            }
        }

        for (name, spec) in &contract.outputs {
            let type_ = match spec {
                OutputSpec::FromModel { type_, .. } | OutputSpec::FromArtifact { type_, .. } => {
                    *type_
                }
            };
            check_output_type(name, &bound[name], type_)?;
        }

        // requireArtifactComplete + node verify.artifactComplete (§3/§4.3).
        let mut completeness: Vec<String> = contract.require_artifact_complete.clone();
        if let Some(v) = &node.verify {
            if let Some(ac) = &v.artifact_complete {
                completeness.push(ac.clone());
            }
        }
        for name in completeness {
            let path = self.find_artifact(&name, &outcome.artifacts)?;
            if !artifacts::is_complete(&path) {
                return Err(TaskFailure::new(
                    ErrorClass::VerificationFailed,
                    format!("required artifact '{name}' is not marked complete"),
                ));
            }
        }

        // Bind into the frame: `vars[node]` exposes outputs both bare
        // (`{{ enrich.summary }}`) and under `.outputs` (`{{ each.outputs... }}`).
        let frame = self.state.stack.last_mut().unwrap();
        let mut node_obj = Map::new();
        for (k, v) in &bound {
            node_obj.insert(k.clone(), v.clone());
        }
        node_obj.insert(
            "outputs".to_string(),
            Value::Object(bound.iter().map(|(k, v)| (k.clone(), v.clone())).collect()),
        );
        vars_obj_mut(frame).insert(node_id.to_string(), Value::Object(node_obj));
        frame.node_outputs.insert(node_id.to_string(), bound);
        Ok(())
    }

    /// Resolve a logical artifact name (e.g. `tracker.list_issues.latest`) to a
    /// path: artifacts produced by this task first, then the run artifact dir.
    fn find_artifact(
        &self,
        name: &str,
        task_artifacts: &BTreeMap<String, PathBuf>,
    ) -> Result<PathBuf, TaskFailure> {
        if let Some(p) = task_artifacts.get(name) {
            return Ok(p.clone());
        }
        if let Ok(entries) = std::fs::read_dir(self.run_dir.artifacts_dir()) {
            for entry in entries.flatten() {
                if artifacts::logical_name(&entry.path()).as_deref() == Some(name) {
                    return Ok(entry.path());
                }
            }
        }
        Err(TaskFailure::new(
            ErrorClass::VerificationFailed,
            format!("declared artifact '{name}' was not produced"),
        ))
    }

    // ------------------------------------------------------------------
    // foreach nodes (§4.4, §4.6)

    fn step_foreach(&mut self, node_id: &str, node: &Node, fp: &FramePlan) -> anyhow::Result<()> {
        if !self.state.foreach_cursors.contains_key(node_id) {
            self.foreach_enter(node_id, node, fp)?;
            if self.state.status != StateStatus::Running {
                return Ok(()); // onEmpty fired a terminal/pause transition
            }
            if !self.state.foreach_cursors.contains_key(node_id) {
                return Ok(()); // transitioned away (empty snapshot)
            }
        }
        let phase = self.state.foreach_cursors[node_id].phase;
        match phase {
            ForeachPhase::Next => self.foreach_next(node_id, node, fp),
            ForeachPhase::InBody => self.foreach_body_done(node_id, node),
            ForeachPhase::InBetween => {
                self.take_child_return(node_id)?;
                let cur = self.state.foreach_cursors.get_mut(node_id).unwrap();
                cur.index += 1;
                cur.phase = ForeachPhase::Next;
                self.persist_snapshot()
            }
        }
    }

    /// First entry: resolve `items`, take the snapshot (enumeration: snapshot
    /// is the only legal value in v1, §3), persist it as an artifact.
    fn foreach_enter(&mut self, node_id: &str, node: &Node, fp: &FramePlan) -> anyhow::Result<()> {
        let binding = node.items.as_ref().expect("validated").from.clone();
        let vars = self
            .state
            .stack
            .last()
            .map(|f| f.vars.clone())
            .unwrap_or(Value::Null);
        let items = match resolve_path(&vars, &binding).cloned() {
            Some(Value::Array(a)) => a,
            Some(other) => {
                self.emit(
                    EventType::Error,
                    json!({ "class": ErrorClass::VerificationFailed.as_str(),
                            "detail": format!("foreach items binding '{binding}' is not a list"),
                            "node": node_id }),
                )?;
                return self.finish_failed(&format!(
                    "foreach '{node_id}': items binding '{binding}' resolved to non-list ({other})"
                ));
            }
            None => {
                self.emit(
                    EventType::Error,
                    json!({ "class": ErrorClass::VerificationFailed.as_str(),
                            "detail": format!("foreach items binding '{binding}' is unbound"),
                            "node": node_id }),
                )?;
                return self.finish_failed(&format!(
                    "foreach '{node_id}': items binding '{binding}' is unbound"
                ));
            }
        };

        // Snapshot the items once, into the run artifact store (§4.4
        // foreachCursors snapshotRef).
        let mut buf = String::new();
        for item in &items {
            buf.push_str(&item.to_string());
            buf.push('\n');
        }
        let file_name = format!("{node_id}.snapshot.jsonl");
        let written =
            artifacts::write_atomic(&self.run_dir.artifacts_dir(), &file_name, buf.as_bytes())
                .map_err(|e| anyhow!("snapshot artifact for foreach '{node_id}': {e}"))?;
        self.emit(
            EventType::ArtifactWritten,
            json!({ "path": written.path, "bytes": written.bytes, "sha256": written.sha256 }),
        )?;
        if let Err(failure) = self.account_artifact_bytes(written.bytes, fp) {
            if failure.class == ErrorClass::BudgetExhausted {
                return self.apply_run_limit(fp, &failure.detail);
            }
            return Err(anyhow!(failure.detail));
        }

        if items.is_empty() {
            // onEmpty fires when the snapshot length is 0 (§4.6).
            let t = node.on_empty.clone().expect("validated");
            return self.transit("onEmpty", &t);
        }

        self.state.foreach_cursors.insert(
            node_id.to_string(),
            ForeachCursor {
                index: 0,
                snapshot_ref: format!("artifacts/{file_name}"),
                session_items: 0,
                results: Vec::new(),
                phase: ForeachPhase::Next,
            },
        );
        self.persist_snapshot()?;
        Ok(())
    }

    /// Phase Next: finish, hit the item limit, or push the body frame.
    fn foreach_next(&mut self, node_id: &str, node: &Node, fp: &FramePlan) -> anyhow::Result<()> {
        let items = self.read_snapshot(node_id)?;
        let (index, session_items) = {
            let cur = &self.state.foreach_cursors[node_id];
            (cur.index, cur.session_items)
        };

        if index >= items.len() {
            // Aggregate per §4.6 and transition onComplete: each declared
            // output name collects `body.outputs` across iterations.
            let results = self.state.foreach_cursors[node_id].results.clone();
            let frame = self.state.stack.last_mut().unwrap();
            let mut outputs_obj = Map::new();
            match &node.outputs {
                Some(NodeOutputs::Foreach(spec)) => {
                    for name in spec.keys() {
                        outputs_obj.insert(name.clone(), Value::Array(results.clone()));
                    }
                }
                _ => {
                    outputs_obj.insert("results".to_string(), Value::Array(results));
                }
            }
            let mut node_obj = Map::new();
            node_obj.insert("outputs".to_string(), Value::Object(outputs_obj));
            vars_obj_mut(frame).insert(node_id.to_string(), Value::Object(node_obj));
            let t = node.on_complete.clone().expect("validated");
            return self.transit("onComplete", &t);
        }

        if let Some(max) = node.max_items {
            if session_items >= max {
                let t = node.on_limit.clone().expect("validated");
                return self.transit("onLimit", &t);
            }
        }

        // Push the body frame (§4.4).
        let item = items[index].clone();
        {
            let frame = self.state.stack.last_mut().unwrap();
            vars_obj_mut(frame).insert(node.bind_as.clone().expect("validated"), item);
        }
        let body = node.body.clone().expect("validated");
        let child_vars = self.resolve_inputs(&body.inputs)?;
        let child_pin = self.subflow_pin(&body.flow_ref)?;
        let child_entry = self
            .plan
            .find_plan(&body.flow_ref)
            .expect("validated")
            .entry_step
            .clone();
        self.emit(
            EventType::Transition,
            json!({ "plan": fp.id, "from": node_id, "to": format!("{}:{}", body.flow_ref, child_entry), "key": "body" }),
        )?;
        self.state.stack.push(Frame {
            plan: child_pin,
            node: child_entry.clone(),
            vars: Value::Object(child_vars),
            node_outputs: BTreeMap::new(),
            counters: Counters {
                started_at: now_ms(),
                ..Counters::default()
            },
        });
        self.node_retries = 0;
        self.emit(
            EventType::NodeEntered,
            json!({ "plan": body.flow_ref, "node": child_entry }),
        )?;
        self.state.foreach_cursors.get_mut(node_id).unwrap().phase = ForeachPhase::InBody;
        self.persist_snapshot()?;
        Ok(())
    }

    /// Phase InBody: the body frame just popped; aggregate its outputs (§4.6),
    /// then push the between frame if declared.
    fn foreach_body_done(&mut self, node_id: &str, node: &Node) -> anyhow::Result<()> {
        let returned = self.take_child_return(node_id)?;
        let outputs = returned.outputs;
        let outcome = returned.outcome;
        let items = self.read_snapshot(node_id)?;
        // `indexBy: itemKey` (§3) means: index each result entry by the value
        // of the item's itemKey field. Any other literal names the item field
        // directly.
        let index_field = match &node.outputs {
            Some(NodeOutputs::Foreach(spec)) => spec
                .values()
                .next()
                .map(|c| c.index_by.clone())
                .unwrap_or_else(|| "itemKey".to_string()),
            _ => "itemKey".to_string(),
        };
        let item_key = node.item_key.clone().expect("validated");
        let effective_field = if index_field == "itemKey" {
            item_key.clone()
        } else {
            index_field
        };
        let (index, new_index_done) = {
            let cur = &self.state.foreach_cursors[node_id];
            (cur.index, cur.index + 1)
        };
        let key_value = items
            .get(index)
            .and_then(|it| it.get(&effective_field))
            .cloned()
            .unwrap_or(Value::Null);
        {
            let cur = self.state.foreach_cursors.get_mut(node_id).unwrap();
            let mut entry = Map::new();
            entry.insert(effective_field, key_value);
            entry.insert("outcome".to_string(), Value::String(outcome));
            entry.insert("outputs".to_string(), serde_json::to_value(&outputs)?);
            cur.results.push(Value::Object(entry));
            cur.session_items += 1;
        }

        if let Some(between) = node.between.clone() {
            // The between flow sees the foreach progress as
            // `{{ <foreachId>.cursor }}` / `{{ <foreachId>.snapshot_len }}`.
            let mut child_vars = Map::new();
            child_vars.insert(
                node_id.to_string(),
                json!({ "cursor": new_index_done, "snapshot_len": items.len() }),
            );
            let child_pin = self.subflow_pin(&between.flow_ref)?;
            let child_entry = self
                .plan
                .find_plan(&between.flow_ref)
                .expect("validated")
                .entry_step
                .clone();
            let (plan_id, _) = self.current_position()?;
            self.emit(
                EventType::Transition,
                json!({ "plan": plan_id, "from": node_id, "to": format!("{}:{}", between.flow_ref, child_entry), "key": "between" }),
            )?;
            self.state.stack.push(Frame {
                plan: child_pin,
                node: child_entry.clone(),
                vars: Value::Object(child_vars),
                node_outputs: BTreeMap::new(),
                counters: Counters {
                    started_at: now_ms(),
                    ..Counters::default()
                },
            });
            self.node_retries = 0;
            self.emit(
                EventType::NodeEntered,
                json!({ "plan": between.flow_ref, "node": child_entry }),
            )?;
            self.state.foreach_cursors.get_mut(node_id).unwrap().phase = ForeachPhase::InBetween;
            self.persist_snapshot()?;
            Ok(())
        } else {
            let cur = self.state.foreach_cursors.get_mut(node_id).unwrap();
            cur.index += 1;
            cur.phase = ForeachPhase::Next;
            self.persist_snapshot()
        }
    }

    fn take_child_return(&mut self, node_id: &str) -> anyhow::Result<PendingReturn> {
        let parent = self
            .state
            .stack
            .last()
            .ok_or_else(|| anyhow!("missing parent frame"))?;
        let returned = self.state.pending_return.as_ref().ok_or_else(|| {
            anyhow!("invalid recovery state: missing child return for '{node_id}'")
        })?;
        if returned.parent_plan != parent.plan || returned.parent_node != node_id {
            return Err(anyhow!(
                "invalid recovery state: child return belongs to another parent"
            ));
        }
        Ok(self.state.pending_return.take().unwrap())
    }

    fn read_snapshot(&self, node_id: &str) -> anyhow::Result<Vec<Value>> {
        let cur = self
            .state
            .foreach_cursors
            .get(node_id)
            .ok_or_else(|| anyhow!("no foreach cursor for '{node_id}'"))?;
        let path = self.run_dir.path().join(&cur.snapshot_ref);
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("reading foreach snapshot {}", path.display()))?;
        let mut items = Vec::new();
        for line in text.lines() {
            if !line.trim().is_empty() {
                items.push(serde_json::from_str(line)?);
            }
        }
        Ok(items)
    }

    fn resolve_inputs(
        &self,
        inputs: &BTreeMap<String, FromBinding>,
    ) -> anyhow::Result<Map<String, Value>> {
        let vars = self
            .state
            .stack
            .last()
            .map(|f| f.vars.clone())
            .unwrap_or(Value::Null);
        let mut out = Map::new();
        for (name, binding) in inputs {
            let v = resolve_path(&vars, &binding.from).cloned().ok_or_else(|| {
                anyhow!(
                    "flowRef input '{name}': binding '{}' is unbound",
                    binding.from
                )
            })?;
            out.insert(name.clone(), v);
        }
        Ok(out)
    }

    fn subflow_pin(&self, flow_ref: &str) -> anyhow::Result<String> {
        self.plan
            .find_plan(flow_ref)
            .map(|p| p.pin())
            .ok_or_else(|| anyhow!("flowRef '{flow_ref}' is not loaded"))
    }

    // ------------------------------------------------------------------
    // mutation journaling crash recovery (§6)

    /// On resume, an intent without a result triggers the task's onUncertain
    /// policy (§6).
    fn handle_recovery(&mut self) -> anyhow::Result<()> {
        // The snapshot watermark identifies committed node/cursor boundaries,
        // even when this node is legitimately revisited. A completed mutation
        // outside that prefix has no resumable generation state: do not replay.
        if self.unsafe_completed_replay {
            self.finish_failed("completed mutation beyond snapshot boundary: unsafe replay")
        } else {
            self.handle_uncertain()
        }
    }

    fn handle_uncertain(&mut self) -> anyhow::Result<()> {
        let pending = std::mem::take(&mut self.pending_uncertain);
        if let Some(intent) = pending.into_iter().next() {
            let policy = self
                .registry
                .task(&intent.task_ref)
                .and_then(|t| t.mutations.as_ref())
                .map(|m| m.on_uncertain);
            let detail = match policy {
                Some(OnUncertain::RefetchState) =>
                    "onUncertain=refetch-state: reconciliation is not implemented; write may have applied",
                Some(OnUncertain::FailRun) | None => "onUncertain=fail-run: write may have applied",
            };
            let reason = format!(
                "uncertain mutation {}.{} ({detail})",
                intent.server, intent.tool
            );
            self.emit(
                EventType::Error,
                json!({ "class": ErrorClass::McpError.as_str(), "detail": reason, "node": intent.node }),
            )?;
            return self.finish_failed(&reason);
        }
        Ok(())
    }
}

fn collect_pins(plan: &ExecutionPlan, out: &mut Vec<(String, String)>) {
    out.push((plan.id.clone(), plan.sha256.clone()));
    for sub in plan.subflows.values() {
        collect_pins(sub, out);
    }
}

pub fn plan_id_of_pin(pin: &str) -> String {
    pin.split('@').next().unwrap_or(pin).to_string()
}

fn fp_node<'a>(root: &'a ExecutionPlan, plan_id: &str, node_id: &str) -> anyhow::Result<&'a Node> {
    root.find_plan(plan_id)
        .and_then(|p| p.nodes.get(node_id))
        .ok_or_else(|| anyhow!("node '{node_id}' not found in plan '{plan_id}'"))
}

fn vars_obj_mut(frame: &mut Frame) -> &mut Map<String, Value> {
    if !frame.vars.is_object() {
        frame.vars = Value::Object(Map::new());
    }
    frame.vars.as_object_mut().unwrap()
}

/// Resolve a dotted binding path (`setup.outputs.items`, `item`) against vars.
fn resolve_path<'a>(vars: &'a Value, path: &str) -> Option<&'a Value> {
    let mut cur = vars;
    for part in path.split('.') {
        cur = cur.get(part)?;
    }
    Some(cur)
}

fn check_output_type(name: &str, v: &Value, ty: OutputType) -> Result<(), TaskFailure> {
    let ok = match ty {
        OutputType::Prose | OutputType::String => v.is_string(),
        OutputType::List => v.is_array(),
        OutputType::Object => v.is_object(),
        OutputType::Number => v.is_number(),
        OutputType::Bool => v.is_boolean(),
    };
    if !ok {
        return Err(TaskFailure::new(
            ErrorClass::VerificationFailed,
            format!("output '{name}' has wrong type (expected {ty:?}, got {v})"),
        ));
    }
    Ok(())
}

fn parse_artifact(format: &ArtifactFormat, bytes: &[u8]) -> anyhow::Result<Value> {
    let text = String::from_utf8(bytes.to_vec()).context("artifact is not UTF-8")?;
    match format {
        ArtifactFormat::Jsonl => {
            let mut items = Vec::new();
            for line in text.lines() {
                if !line.trim().is_empty() {
                    items.push(serde_json::from_str::<Value>(line)?);
                }
            }
            Ok(Value::Array(items))
        }
        ArtifactFormat::Json => Ok(serde_json::from_str(&text)?),
    }
}

#[cfg(test)]
mod recovery_regressions {
    use super::*;
    use trollm_mcp::{ToolCall, ToolDescriptor, ToolResult};
    use trollm_models::MockModel;

    #[test]
    fn streaming_argument_hash_matches_serialized_json() {
        let value =
            json!({"unicode":"עברית 🦀", "escaped":"\"\\\n\0", "nested":[null, 42, {"key":true}]});
        assert_eq!(
            hash_json(&value).unwrap(),
            hex_sha256(value.to_string().as_bytes())
        );
    }

    struct NoGateway;

    struct CancelControl;
    impl ExecutionControl for CancelControl {
        fn checkpoint(&self) -> bool {
            true
        }
        fn wait_backoff(&self, _: Duration) -> bool {
            true
        }
    }
    impl ToolGateway for NoGateway {
        fn tool_class(&self, _: &str, _: &str) -> trollm_mcp::policy::ToolClass {
            trollm_mcp::policy::ToolClass::Read
        }
        fn list_tools(&self) -> anyhow::Result<Vec<ToolDescriptor>> {
            Ok(vec![])
        }
        fn validate_call(&self, _: &ToolCall) -> anyhow::Result<()> {
            anyhow::bail!("no tools supported by NoGateway")
        }
        fn call(&mut self, _: &ToolCall) -> anyhow::Result<ToolResult> {
            panic!("unexpected tool effect")
        }
    }

    fn plan(id: &str, entry: &str, nodes: &str) -> ExecutionPlan {
        ExecutionPlan::from_yaml_str(&format!(r#"
schemaVersion: ledger.execution-plan.v1
id: {id}
version: 1
sourceBaseline: {{ sha256: "{}" }}
input: {{ promptTemplate: "" }}
limits: {{ maxSteps: 50, maxReasonerCalls: 0, maxTokensTotal: 10000, timeoutS: 600, artifactMaxMb: 5 }}
onLimit: abort
models: {{ coder: {{ ctx: 8192 }} }}
mcp: {{ servers: [], policies: {{}} }}
entryStep: {entry}
nodes:
{nodes}
"#, "0".repeat(64))).unwrap()
    }

    fn foreach_nodes(node: &str, body: &str, between: bool) -> String {
        format!(
            r#"  {node}:
    kind: foreach
    items: {{ from: items }}
    itemKey: id
    bindAs: item
    enumeration: snapshot
    ordering: source
    body: {{ flowRef: {body}, inputs: {{ items: {{ from: item.children }} }} }}
{}
    outputs: {{ results: {{ collect: body.outputs, indexBy: itemKey }} }}
    onEmpty: done
    onComplete: done
  done: {{ kind: end, outcome: succeeded }}"#,
            if between {
                "    between: { flowRef: between }"
            } else {
                ""
            }
        )
    }

    fn executor(plan: ExecutionPlan, run: RunDir) -> Executor<MockModel, NoGateway> {
        Executor::new(
            plan,
            TaskRegistry::from_yaml_str(
                "schemaVersion: ledger.task-registry.v1\ntasks: {}",
                Path::new("."),
            )
            .unwrap(),
            String::new(),
            SlotMap::new().with("coder", MockModel::default()),
            NoGateway,
            run,
        )
        .unwrap()
    }

    #[test]
    fn streamed_recovery_preserves_future_intent_answers_and_missing_intent_results() {
        for future_intent in [false, true] {
            let base = std::env::temp_dir().join(format!("streamed-recovery-{}", Uuid::new_v4()));
            let run = RunDir::under(&base);
            let root = plan(
                "streamed-recovery",
                "done",
                "  done: { kind: end, outcome: succeeded }",
            );
            let mut original = executor(root.clone(), run.clone());
            original
                .log
                .append(
                    EventType::ToolCallResult,
                    json!({"intentSeq": if future_intent { 2 } else { 77 }}),
                )
                .unwrap();
            if future_intent {
                original.log.append(EventType::ToolCallIntent,
                    json!({"node":"done", "taskRef":"old-task", "server":"t", "tool":"write", "args":{}})).unwrap();
                // Both historical records are committed. An answer occurring
                // before its referenced intent must still suppress uncertainty.
                original.state.event_watermark = 2;
            }
            original.write_snapshot().unwrap();
            drop(original);
            let mut restored = executor(root, run);
            assert!(restored.pending_uncertain.is_empty());
            assert_eq!(restored.unsafe_completed_replay, !future_intent);
            let status = restored.resume().unwrap();
            if future_intent {
                assert_eq!(status, RunStatus::Succeeded);
            } else {
                // A completed result past the watermark blocks execution even
                // when no record for its referenced intent exists in the log.
                assert!(
                    matches!(status, RunStatus::Failed { reason } if reason.contains("unsafe replay"))
                );
                assert_eq!(restored.state.counters.steps, 0);
            }
            drop(restored);
            std::fs::remove_dir_all(base).unwrap();
        }
    }

    #[test]
    fn cancelled_task_keeps_artifact_io_error_without_terminal_claim() {
        // The actual EventLog append failure is not injected here; this covers
        // its classified error at the handler boundary without a logger seam.
        let base = std::env::temp_dir().join(format!("cancel-io-{}", Uuid::new_v4()));
        let mut exec = executor(
            plan(
                "cancel-io",
                "done",
                "  done: { kind: end, outcome: succeeded }",
            ),
            RunDir::under(&base),
        );
        exec.set_control(Arc::new(CancelControl));
        let node = exec.plan.nodes.get("done").unwrap().clone();
        let frame = exec.frame_plan("cancel-io").unwrap();
        let error = exec
            .handle_task_error(
                "done",
                &node,
                &frame,
                TaskFailure::new(ErrorClass::ArtifactIo, "journal append failed"),
            )
            .unwrap_err();
        assert!(error.to_string().contains("journal append failed"));
        assert_eq!(exec.state.status, StateStatus::Running);
        let events = EventLog::read_all(&exec.run_dir.events_path()).unwrap();
        assert!(!events.iter().any(|event| event["type"] == "run_ended"));
        drop(exec);
        std::fs::remove_dir_all(base).unwrap();
    }

    fn advance_foreach(exec: &mut Executor<MockModel, NoGateway>, id: &str) {
        let (pid, _) = exec.current_position().unwrap();
        let node = fp_node(&exec.plan, &pid, id).unwrap().clone();
        let fp = exec.frame_plan(&pid).unwrap();
        exec.step_foreach(id, &node, &fp).unwrap();
    }

    #[test]
    fn child_return_survives_pop_aggregation_and_between_snapshots() {
        for boundary in 0..3 {
            let mut root = plan("root", "each", &foreach_nodes("each", "child", true));
            root.subflows.insert(
                "child".into(),
                plan("child", "done", "  done: { kind: end, outcome: succeeded }"),
            );
            root.subflows.insert(
                "between".into(),
                plan(
                    "between",
                    "done",
                    "  done: { kind: end, outcome: succeeded }",
                ),
            );
            let run =
                RunDir::under(&std::env::temp_dir().join(format!("return-{}", Uuid::new_v4())));
            let mut exec = executor(root.clone(), run.clone());
            exec.state.stack[0].vars = json!({"items": [{"id": 7, "children": []}]});
            advance_foreach(&mut exec, "each");
            exec.state.stack.last_mut().unwrap().node_outputs.insert(
                "work".into(),
                BTreeMap::from([("note".into(), json!("durable"))]),
            );
            let fp = exec.frame_plan("child").unwrap();
            let end = fp_node(&exec.plan, "child", "done").unwrap().clone();
            exec.step_end("done", &end, &fp).unwrap(); // published pop boundary
            if boundary > 0 {
                advance_foreach(&mut exec, "each");
            } // aggregation + between push
            if boundary > 1 {
                let fp = exec.frame_plan("between").unwrap();
                let end = fp_node(&exec.plan, "between", "done").unwrap().clone();
                exec.step_end("done", &end, &fp).unwrap(); // between pop
            }
            drop(exec);
            let mut resumed = executor(root, run.clone());
            assert_eq!(resumed.resume().unwrap(), RunStatus::Succeeded);
            let results = &resumed.state.stack[0].vars["each"]["outputs"]["results"];
            assert_eq!(results.as_array().unwrap().len(), 1);
            assert_eq!(results[0]["outcome"], "succeeded");
            assert_eq!(results[0]["outputs"]["work"]["note"], "durable");
            drop(resumed);
            std::fs::remove_dir_all(run.path().parent().unwrap()).unwrap();
        }
    }

    #[test]
    fn missing_child_return_is_invalid_recovery_state() {
        let mut root = plan("root", "each", &foreach_nodes("each", "child", false));
        root.subflows.insert(
            "child".into(),
            plan("child", "done", "  done: { kind: end, outcome: succeeded }"),
        );
        let run =
            RunDir::under(&std::env::temp_dir().join(format!("missing-return-{}", Uuid::new_v4())));
        let mut exec = executor(root, run.clone());
        exec.state.stack[0].vars = json!({"items": [{"id": 7, "children": []}]});
        advance_foreach(&mut exec, "each");
        exec.state.stack.pop();
        assert!(exec
            .foreach_body_done("each", &exec.plan.nodes["each"].clone())
            .unwrap_err()
            .to_string()
            .contains("missing child return"));
        drop(exec);
        std::fs::remove_dir_all(run.path().parent().unwrap()).unwrap();
    }

    #[test]
    fn nested_body_and_between_lookup_and_deepest_resume() {
        let mut root = plan("root", "outer", &foreach_nodes("outer", "child", false));
        let mut child = plan("child", "inner", &foreach_nodes("inner", "leaf", true));
        child.subflows.insert(
            "leaf".into(),
            plan("leaf", "done", "  done: { kind: end, outcome: succeeded }"),
        );
        child.subflows.insert(
            "between".into(),
            plan(
                "between",
                "done",
                "  done: { kind: end, outcome: succeeded }",
            ),
        );
        root.subflows.insert("child".into(), child);
        let run = RunDir::under(&std::env::temp_dir().join(format!("nested-{}", Uuid::new_v4())));
        let mut exec = executor(root.clone(), run.clone());
        exec.state.stack[0].vars =
            json!({"items": [{"id": 1, "children": [{"id": 2, "children": []}]}]});
        advance_foreach(&mut exec, "outer");
        advance_foreach(&mut exec, "inner");
        assert_eq!(exec.state.stack.len(), 3);
        drop(exec);
        let mut resumed = executor(root.clone(), run.clone());
        assert_eq!(resumed.resume().unwrap(), RunStatus::Succeeded);
        assert_eq!(
            resumed.state.foreach_cursors["inner"].results[0]["outcome"],
            "succeeded"
        );
        root.subflows.insert(
            "duplicate".into(),
            plan("leaf", "done", "  done: { kind: end, outcome: succeeded }"),
        );
        assert!(validate::validate_plan(&root)
            .unwrap_err()
            .to_string()
            .contains("duplicate plan id"));
        drop(resumed);
        std::fs::remove_dir_all(run.path().parent().unwrap()).unwrap();
    }
    #[test]
    fn final_output_overrides_enforce_types_and_completeness() {
        for (binding, complete, expected) in [
            (
                "items: { from: model }",
                true,
                RunStatus::Failed {
                    reason: "failed".into(),
                },
            ),
            (
                "items: { fromArtifact: replacement, format: jsonl }",
                false,
                RunStatus::Failed {
                    reason: "failed".into(),
                },
            ),
            (
                "items: { fromArtifact: replacement, format: jsonl }",
                true,
                RunStatus::Succeeded,
            ),
            (
                "extra: { fromArtifact: replacement, format: jsonl }",
                false,
                RunStatus::Failed {
                    reason: "failed".into(),
                },
            ),
        ] {
            let root = plan(
                "outputs",
                "work",
                &format!(
                    r#"  work:
    kind: task
    taskRef: work
    outputs: {{ {binding} }}
    onVerified: success
    onError: {{ then: failure }}
  success: {{ kind: end, outcome: succeeded }}
  failure: {{ kind: end, outcome: failed }}"#
                ),
            );
            let registry = TaskRegistry::from_yaml_str(
                r#"schemaVersion: ledger.task-registry.v1
 tasks:
  work:
    model: coder
    prompt: work.md
    tools: []
    outputContract:
      outputs:
        items: { fromArtifact: original, format: jsonl, type: list }
      allowNoop: true
"#
                .trim_start()
                .replace("\n tasks:", "\ntasks:")
                .as_str(),
                Path::new("."),
            )
            .unwrap();
            let base = std::env::temp_dir().join(format!("outputs-{}", Uuid::new_v4()));
            std::fs::create_dir_all(&base).unwrap();
            std::fs::write(base.join("work.md"), "work").unwrap();
            let mut registry = registry;
            registry.base_dir = Some(base.clone());
            let run = RunDir::under(&base);
            run.create_dirs().unwrap();
            for (name, finished) in [("original", true), ("replacement", complete)] {
                let art = artifacts::write_atomic(
                    &run.artifacts_dir(),
                    &format!("{name}.jsonl"),
                    b"{\"id\":1}\n",
                )
                .unwrap();
                artifacts::write_meta(
                    &art.path,
                    &artifacts::ArtifactMeta {
                        fetched_at: 0,
                        complete: finished,
                    },
                )
                .unwrap();
            }
            let mut exec = Executor::new(
                root,
                registry,
                String::new(),
                SlotMap::new().with("coder", MockModel::scripted(["prose"])),
                NoGateway,
                run.clone(),
            )
            .unwrap();
            assert_eq!(
                exec.run_until_blocked().unwrap(),
                expected,
                "binding={binding}, complete={complete}"
            );
            if expected == RunStatus::Succeeded {
                assert!(exec.state.stack[0].vars["work"]["outputs"]["items"].is_array());
            }
            drop(exec);
            std::fs::remove_dir_all(base).unwrap();
        }
    }

    #[test]
    fn executor_uses_models_grammar_from_actual_task_grants() {
        let base = std::env::temp_dir().join(format!("grammar-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&base).unwrap();
        std::fs::write(base.join("work.md"), "work").unwrap();
        let root = plan("grammar", "work", "  work: { kind: task, taskRef: work, onVerified: done }\n  done: { kind: end, outcome: succeeded }");
        let registry = TaskRegistry::from_yaml_str("schemaVersion: ledger.task-registry.v1\ntasks:\n  work:\n    model: coder\n    prompt: work.md\n    tools: [tracker.read]\n    outputContract: { allowNoop: true }", &base).unwrap();
        let mut exec = Executor::new(
            root,
            registry,
            String::new(),
            SlotMap::new().with("coder", MockModel::scripted(["prose"])),
            NoGateway,
            RunDir::under(&base),
        )
        .unwrap();
        assert_eq!(exec.run_until_blocked().unwrap(), RunStatus::Succeeded);
        let requests = exec.models.get_mut("coder").unwrap().recorded_requests();
        assert_eq!(
            requests[0].grammar_gbnf,
            Some(trollm_models::fence::tool_call_grammar(&[(
                "tracker".into(),
                "read".into()
            )]))
        );
        drop(exec);
        std::fs::remove_dir_all(base).unwrap();
    }

    fn budget_events(exec: &Executor<MockModel, NoGateway>) -> Vec<Value> {
        std::fs::read_to_string(exec.run_dir.events_path())
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    #[test]
    fn artifact_allowance_excludes_parent_and_previous_siblings() {
        let root = plan("root", "done", "  done: { kind: end, outcome: succeeded }");
        let run =
            RunDir::under(&std::env::temp_dir().join(format!("frame-bytes-{}", Uuid::new_v4())));
        let mut exec = executor(root, run.clone());
        let mut fp = exec.frame_plan("root").unwrap();
        fp.limits.artifact_max_mb = 1;
        let mib = 1024 * 1024;
        exec.account_artifact_bytes(mib, &fp).unwrap(); // parent at exact limit
        let parent = exec.state.stack[0].clone();
        let mut child = parent.clone();
        child.counters = Counters::default();
        exec.state.stack.push(child.clone());
        exec.account_artifact_bytes(mib, &fp).unwrap(); // first child exact limit
        exec.state.stack.pop();
        exec.state.stack.push(child);
        exec.account_artifact_bytes(mib, &fp).unwrap(); // sibling exact limit
        assert_eq!(exec.state.counters.artifact_bytes, 3 * mib);
        assert_eq!(exec.state.stack[0].counters.artifact_bytes, mib);
        assert!(!budget_events(&exec)
            .iter()
            .any(|e| e["type"] == "budget_exhausted"));
        let failure = exec.account_artifact_bytes(1, &fp).unwrap_err();
        assert_eq!(failure.class, ErrorClass::BudgetExhausted);
        assert_eq!(exec.state.counters.artifact_bytes, 3 * mib + 1);
        assert_eq!(
            exec.state.stack.last().unwrap().counters.artifact_bytes,
            mib + 1
        );
        let events = budget_events(&exec);
        let event = events
            .iter()
            .find(|e| e["type"] == "budget_exhausted")
            .unwrap();
        assert_eq!(event["budget"], "artifactMaxMb");
        assert_eq!(event["value"], mib + 1);
        assert_eq!(event["limit"], mib);
        drop(exec);
        std::fs::remove_dir_all(run.path().parent().unwrap()).unwrap();
    }

    #[test]
    fn unrepresentably_large_artifact_allowance_saturates() {
        let root = plan("root", "done", "  done: { kind: end, outcome: succeeded }");
        let run = RunDir::under(
            &std::env::temp_dir().join(format!("large-artifact-limit-{}", Uuid::new_v4())),
        );
        let mut exec = executor(root, run.clone());
        let mut fp = exec.frame_plan("root").unwrap();
        fp.limits.artifact_max_mb = u64::MAX;
        exec.account_artifact_bytes(1, &fp).unwrap();
        assert_eq!(exec.state.counters.artifact_bytes, 1);
        assert_eq!(exec.state.stack[0].counters.artifact_bytes, 1);
        assert!(!budget_events(&exec)
            .iter()
            .any(|e| e["type"] == "budget_exhausted"));
        drop(exec);
        std::fs::remove_dir_all(run.path().parent().unwrap()).unwrap();
    }

    #[test]
    fn foreach_snapshot_alone_charges_owning_frame_and_obeys_on_limit() {
        for policy in [RunLimitPolicy::Abort, RunLimitPolicy::RespondPartial] {
            for items in [json!([]), json!([{"id": 1}])] {
                let mut root = plan("root", "each", &foreach_nodes("each", "child", false));
                root.subflows.insert(
                    "child".into(),
                    plan("child", "done", "  done: { kind: end, outcome: succeeded }"),
                );
                root.limits.artifact_max_mb = 0;
                root.on_limit = policy;
                let run = RunDir::under(
                    &std::env::temp_dir().join(format!("snapshot-budget-{}", Uuid::new_v4())),
                );
                let mut exec = executor(root, run.clone());
                exec.state.stack[0].vars = json!({"items": items});
                assert_eq!(
                    exec.run_until_blocked().unwrap(),
                    if items.as_array().unwrap().is_empty()
                        || matches!(policy, RunLimitPolicy::RespondPartial)
                    {
                        RunStatus::Succeeded
                    } else {
                        RunStatus::Failed {
                            reason: "budget-exhausted: artifactMaxMb exceeded".into(),
                        }
                    }
                );
                let events = budget_events(&exec);
                let written = events
                    .iter()
                    .find(|e| e["type"] == "artifact_written")
                    .unwrap()["bytes"]
                    .as_u64()
                    .unwrap();
                assert_eq!(exec.state.counters.artifact_bytes, written);
                assert_eq!(exec.state.stack[0].counters.artifact_bytes, written);
                let exhausted: Vec<_> = events
                    .iter()
                    .filter(|e| e["type"] == "budget_exhausted")
                    .collect();
                if written == 0 {
                    assert!(exhausted.is_empty());
                    assert_eq!(events.last().unwrap()["outcome"], "succeeded");
                } else {
                    assert_eq!(exhausted.len(), 1);
                    assert_eq!(exhausted[0]["budget"], "artifactMaxMb");
                    assert_eq!(
                        events.last().unwrap()["outcome"],
                        if matches!(policy, RunLimitPolicy::Abort) {
                            "failed"
                        } else {
                            "partial"
                        }
                    );
                }
                drop(exec);
                std::fs::remove_dir_all(run.path().parent().unwrap()).unwrap();
            }
        }
    }

    #[test]
    fn timeout_at_task_entry_exact_zero_and_over_obey_on_limit() {
        for policy in [RunLimitPolicy::Abort, RunLimitPolicy::RespondPartial] {
            for expired in [false, true] {
                let base = std::env::temp_dir().join(format!("timeout-budget-{}", Uuid::new_v4()));
                std::fs::create_dir_all(&base).unwrap();
                std::fs::write(base.join("prompt.md"), "go").unwrap();
                let mut root = plan("root", "work", "  work: { kind: task, taskRef: work, onVerified: done }\n  done: { kind: end, outcome: succeeded }");
                root.limits.timeout_s = 0;
                root.on_limit = policy;
                let run = RunDir::under(&base);
                let registry = TaskRegistry::from_yaml_str("schemaVersion: ledger.task-registry.v1\ntasks:\n  work:\n    model: coder\n    prompt: prompt.md\n    tools: []\n    outputContract: { outputs: { summary: { from: model, type: prose } } }", &base).unwrap();
                let mut exec = Executor::new(
                    root,
                    registry,
                    String::new(),
                    SlotMap::new().with("coder", MockModel::scripted(["done"])),
                    NoGateway,
                    run,
                )
                .unwrap();
                // Future persisted start saturates elapsed to exactly zero without
                // sleeping or introducing a production clock abstraction.
                exec.state.stack[0].counters.started_at = if expired { 0 } else { u64::MAX };
                let status = exec.run_until_blocked().unwrap();
                let events = budget_events(&exec);
                let exhausted: Vec<_> = events
                    .iter()
                    .filter(|e| e["type"] == "budget_exhausted")
                    .collect();
                if expired {
                    assert_eq!(exec.state.counters.steps, 0);
                    assert_eq!(exhausted.len(), 1);
                    assert_eq!(exhausted[0]["budget"], "timeoutS");
                    assert_eq!(exhausted[0]["limit"], 0);
                    assert_eq!(
                        matches!(status, RunStatus::Failed { .. }),
                        matches!(policy, RunLimitPolicy::Abort)
                    );
                    assert_eq!(
                        events.last().unwrap()["outcome"],
                        if matches!(policy, RunLimitPolicy::Abort) {
                            "failed"
                        } else {
                            "partial"
                        }
                    );
                } else {
                    assert_eq!(status, RunStatus::Succeeded);
                    assert_eq!(exec.state.counters.steps, 1);
                    assert!(exhausted.is_empty());
                }
                drop(exec);
                std::fs::remove_dir_all(base).unwrap();
            }
        }
    }
}
