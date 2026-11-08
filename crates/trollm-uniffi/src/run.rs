//! Run management: one background `std::thread` per run owning its
//! `Executor` (the executor is `Send` but not `Sync`; the worker thread is
//! the only thread that ever touches it). Cross-thread control is by flags +
//! a command channel:
//!
//! - **plan-driven pause/resume** (§4.4: `maxItems → paused` etc.): when
//!   `run_until_blocked` returns `Paused`, the worker parks on a channel;
//!   `resume`/`cancel` are delivered to the parked worker and invoke the
//!   executor's own `resume()`/`cancel()` — exact core semantics.
//! - **user-initiated pause of a *running* run**: cooperative hold. The
//!   wrapper's model slot / gateway (`Controlled*`) park the worker at the
//!   next model/tool boundary until resumed or cancelled. This does NOT write
//!   a §7 `paused` event and the persisted snapshot still says `running`
//!   (crash while held = ordinary crash recovery). A first-class persisted
//!   user-pause needs a core API (step or injectable control) — reported as
//!   an open question; core is not changed here.
//! - **user-initiated cancel of a *running* run**: core observes the flag at
//!   dispatch and retry boundaries, while the wrappers observe it around
//!   synchronous model/tool calls. Core records `run_ended{outcome:"cancelled"}`
//!   after any available tool result has been journaled.
//!
//! Proxy events (§6): `McpGateway`'s sink cannot append to the run's main
//! `events.jsonl` without a core change — `Executor` owns the log and its
//! `seq` counter, so a second writer would break §7's monotonic `seq`. The
//! sink therefore writes `runs/<uuid>/proxy.events.jsonl` (same envelope
//! shape, native proxy event kinds, sidecar-local seq). Opening validates and
//! continues that sequence; audit failure stops further dispatch. Consolidation
//! remains open; `tail_events` reads the authoritative main log only.

use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};
use std::collections::{HashMap, VecDeque};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};
use trollm_core::exec::{ExecutionControl, SlotMap};
use trollm_core::{EventType, ExecutionPlan, Executor, RunDir, RunStatus, TaskRegistry};
use trollm_mcp::{EventSink, McpGateway, PolicySet, ProxyEvent, RootCertificates, ServerConfig};
use trollm_models::{GenerateRequest, GenerateResponse, ModelSlot};

mod preparation;
use preparation::Preparations;

use crate::authentication::RunAuthentication;
use crate::bundle::{BundleManifest, MANIFEST_NAME};
use crate::configuration::RunConfiguration;
use crate::{RunActivityApi, SharedTokenProvider, TrollmError};

/// Marker error text returned by controlled slots/gateways when the user
/// cancels a running run. Auditable in the event log; never model prose.
pub const CANCEL_MARKER: &str = "__trollm_cancelled__";
pub const WORKER_FAILURE_FILE: &str = "worker-failure.json";

/// What the FFI layer reports for a run (marshalling of core's RunStatus
/// plus the cooperative-hold states above).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunPhase {
    Running,
    Paused { reason: String },
    Succeeded,
    Failed { reason: String },
}

/// Shared control block for one run.
pub struct RunControl {
    pub(crate) observation: Arc<Mutex<crate::observation::Projection>>,
    hold_ns: AtomicU64,
    cancel: AtomicBool,
    cancellation_cause: Mutex<Option<&'static str>>,
    worker_active: AtomicBool,
    selected_preparer: Option<Arc<crate::collection_provider::InstalledCollectionPreparer>>,
    /// Keep the only available failure diagnostic if its sidecar could not be written.
    volatile_worker_failure: AtomicBool,
    activity: Mutex<Option<RunActivityApi>>,
    preparation_activity: Mutex<HashMap<String, RunActivityApi>>,
    pause_requested: AtomicBool,
    gate: Mutex<PauseGate>,
    gate_cond: Condvar,
    status: Arc<Mutex<RunPhase>>,
}

#[derive(Default)]
struct PauseGate {
    /// Set only while the worker is parked at the current authored pause.
    plan_paused: bool,
}

// Cooperative wake cadence only, not a freshness/authority policy or hard-stop
// guarantee. Every wake resamples the selected device clock/original cutoff.
const SELECTED_CONTROL_RECHECK: Duration = Duration::from_millis(100);

impl RunControl {
    pub(crate) fn selected_invocation_binding(&self) -> Result<(String, String)> {
        let preparer = self
            .selected_preparer
            .as_ref()
            .context("worker has no installed selected preparer")?;
        let (key, invocation) = preparer.private_invocation_binding()?;
        Ok((key.to_owned(), invocation.to_owned()))
    }

    pub fn worker_active(&self) -> bool {
        self.worker_active.load(Ordering::SeqCst)
    }

    pub(crate) fn has_volatile_failure(&self) -> bool {
        self.volatile_worker_failure.load(Ordering::SeqCst)
    }

    pub fn activity(&self) -> Option<RunActivityApi> {
        self.activity.lock().unwrap().clone().or_else(|| {
            let background = self.preparation_activity.lock().unwrap();
            background
                .get("coder")
                .or_else(|| background.values().next())
                .cloned()
        })
    }

    fn set_activity(&self, activity: Option<RunActivityApi>) {
        *self.activity.lock().unwrap() = activity;
    }

    /// Called by Controlled{Model,Gateway} at every model/tool boundary.
    pub fn before_boundary(&self) -> Result<()> {
        // The notifier takes this same mutex before changing the predicate.
        // This closes the check-to-wait window for both resume and cancel.
        let mut guard = self
            .gate
            .lock()
            .map_err(|_| anyhow!("run control lock poisoned"))?;
        if self.is_cancelled() {
            bail!(CANCEL_MARKER);
        }
        if self.pause_requested.load(Ordering::SeqCst) {
            let hold_started = Instant::now();
            {
                let mut status = self.status.lock().unwrap();
                *status = RunPhase::Paused {
                    reason: "user-requested (cooperative hold; not a persisted §4.4 pause)"
                        .to_string(),
                };
            }
            while self.pause_requested.load(Ordering::SeqCst) && !self.is_cancelled() {
                guard = if self.selected_preparer.is_some() {
                    self.gate_cond
                        .wait_timeout(guard, SELECTED_CONTROL_RECHECK)
                        .map_err(|_| anyhow!("run control lock poisoned"))?
                        .0
                } else {
                    self.gate_cond
                        .wait(guard)
                        .map_err(|_| anyhow!("run control lock poisoned"))?
                };
            }
            self.record_hold(hold_started);
            *self.status.lock().unwrap() = RunPhase::Running;
        }
        if self.is_cancelled() {
            bail!(CANCEL_MARKER);
        }
        Ok(())
    }

    fn record_hold(&self, started: Instant) {
        let ns = started.elapsed().as_nanos().min(u64::MAX as u128) as u64;
        let _ = self
            .hold_ns
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                Some(value.saturating_add(ns))
            });
    }

    pub fn request_pause(&self) {
        let _guard = self.gate.lock().unwrap();
        self.pause_requested.store(true, Ordering::SeqCst);
        self.gate_cond.notify_all();
    }

    pub fn request_resume(&self) {
        let _guard = self.gate.lock().unwrap();
        self.pause_requested.store(false, Ordering::SeqCst);
        self.gate_cond.notify_all();
    }

    pub fn request_cancel(&self) {
        let mut guard = self.gate.lock().unwrap();
        guard.plan_paused = false;
        self.latch_cancel("OPERATOR_CANCELLED");
        self.pause_requested.store(false, Ordering::SeqCst);
        self.gate_cond.notify_all();
    }

    fn latch_cancel(&self, cause: &'static str) {
        let mut original = self
            .cancellation_cause
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if original.is_none() {
            *original = Some(cause);
        }
        self.cancel.store(true, Ordering::SeqCst);
    }
    pub fn is_cancelled(&self) -> bool {
        if !self.cancel.load(Ordering::SeqCst) {
            if let Some(cause) = self
                .selected_preparer
                .as_ref()
                .and_then(|preparer| preparer.cancellation_cause())
            {
                self.latch_cancel(cause);
            }
        }
        self.cancel.load(Ordering::SeqCst)
    }

    fn wait_backoff_internal(&self, duration: Duration, deadline: Option<Instant>) -> bool {
        let mut remaining = duration;
        let mut guard = self.gate.lock().unwrap();
        loop {
            if self.is_cancelled() {
                return true;
            }
            if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
                return false;
            }
            if self.pause_requested.load(Ordering::SeqCst) {
                let hold_started = Instant::now();
                *self.status.lock().unwrap() = RunPhase::Paused {
                    reason: "user-requested (cooperative hold; not a persisted §4.4 pause)"
                        .to_string(),
                };
                while self.pause_requested.load(Ordering::SeqCst) && !self.is_cancelled() {
                    guard = if self.selected_preparer.is_some() {
                        self.gate_cond
                            .wait_timeout(guard, SELECTED_CONTROL_RECHECK)
                            .unwrap()
                            .0
                    } else {
                        self.gate_cond.wait(guard).unwrap()
                    };
                }
                self.record_hold(hold_started);
                *self.status.lock().unwrap() = RunPhase::Running;
                continue;
            }
            if remaining.is_zero() {
                return false;
            }
            // Bounded chunks avoid `Instant + Duration` overflow for an
            // authored u64::MAX seconds and allow periodic predicate checks.
            let mut chunk = remaining.min(Duration::from_secs(60));
            if self.selected_preparer.is_some() {
                chunk = chunk.min(SELECTED_CONTROL_RECHECK);
            }
            if let Some(deadline) = deadline {
                chunk = chunk.min(deadline.saturating_duration_since(Instant::now()));
            }
            let started = Instant::now();
            let (next, _) = self.gate_cond.wait_timeout(guard, chunk).unwrap();
            guard = next;
            remaining = remaining.saturating_sub(started.elapsed());
        }
    }
}

impl ExecutionControl for RunControl {
    fn observation_hold_ns(&self) -> Option<u64> {
        Some(self.hold_ns.load(Ordering::Relaxed))
    }
    fn checkpoint(&self) -> bool {
        self.before_boundary().is_err()
    }

    fn wait_backoff(&self, duration: Duration) -> bool {
        self.wait_backoff_internal(duration, None)
    }

    fn wait_backoff_until(&self, duration: Duration, deadline: Option<Instant>) -> bool {
        self.wait_backoff_internal(duration, deadline)
    }
}

/// Relative limits include cooperative holds, verification and loading. Each
/// wrapper passes only its remaining allowance to the next layer. A single
/// native load/decode operation remains synchronous and cannot be interrupted.
struct GenerationBudget {
    started: Instant,
    limit: Option<Duration>,
}

impl GenerationBudget {
    fn new(limit: Option<Duration>) -> Self {
        Self {
            started: Instant::now(),
            limit,
        }
    }

    fn remaining(&self) -> Option<Duration> {
        self.limit
            .map(|limit| limit.saturating_sub(self.started.elapsed()))
    }

    fn expired(&self) -> bool {
        self.remaining()
            .is_some_and(|remaining| remaining.is_zero())
    }

    #[cfg(any(feature = "llama", test))]
    fn check(&self) -> Result<()> {
        if self.expired() {
            return Err(GenerationExpired.into());
        }
        Ok(())
    }
}

#[cfg(any(feature = "llama", test))]
#[derive(Debug)]
struct GenerationExpired;

#[cfg(any(feature = "llama", test))]
impl std::fmt::Display for GenerationExpired {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("model generation deadline exceeded before generation")
    }
}

#[cfg(any(feature = "llama", test))]
impl std::error::Error for GenerationExpired {}

fn generation_timeout() -> GenerateResponse {
    GenerateResponse {
        text: String::new(),
        tokens_in: 0,
        tokens_out: 0,
        finish: trollm_models::FinishReason::Timeout,
        capacity: None,
        diagnostics: None,
    }
}

/// ModelSlot wrapper injecting pause/cancel checks around the real slot.
pub struct ControlledModel {
    slot: String,
    inner: Box<dyn ModelSlot>,
    ctl: Arc<RunControl>,
}

impl ModelSlot for ControlledModel {
    fn generate(&mut self, mut req: GenerateRequest) -> Result<GenerateResponse> {
        let budget = GenerationBudget::new(req.max_duration);
        // Clean up inherited activity even when a hold consumes the budget.
        let _activity = ActivityGuard(self.ctl.clone());
        self.ctl.before_boundary()?;
        if budget.expired() {
            return Ok(generation_timeout());
        }
        self.ctl.set_activity(Some(RunActivityApi::Generating {
            slot: self.slot.clone(),
        }));
        req.max_duration = budget.remaining();
        let mut resp = self.inner.generate(req)?;
        self.ctl
            .before_boundary()
            .map_err(|error| match resp.diagnostics.clone() {
                Some(diagnostics) => {
                    trollm_models::ModelDiagnosticError::attach(error, diagnostics)
                }
                None => error,
            })?;
        if budget.expired() {
            resp.finish = trollm_models::FinishReason::Timeout;
        }
        Ok(resp)
    }
}

/// Clears transient activity even when a model returns an error.
struct ActivityGuard(Arc<RunControl>);

impl Drop for ActivityGuard {
    fn drop(&mut self) {
        self.0.set_activity(None);
    }
}

/// Also runs during Rust unwinding: an abandoned worker must not look live.
struct WorkerGuard(Arc<RunControl>);

impl Drop for WorkerGuard {
    fn drop(&mut self) {
        if let Some(preparer) = &self.0.selected_preparer {
            preparer.stop_clock_maintenance();
        }
        self.0.set_activity(None);
        // Readers capture phase then projection in this same order. Release
        // event/report storage even for retained volatile-failure handles; only
        // their irreplaceable diagnostic must survive the inactive cache bound.
        let _phase = self
            .0
            .status
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        self.0.worker_active.store(false, Ordering::SeqCst);
        let mut view = self
            .0
            .observation
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        *view = crate::observation::Projection::default();
    }
}

/// ToolGateway wrapper injecting the same boundary checks.
pub struct ControlledGateway {
    inner: McpGateway,
    ctl: Arc<RunControl>,
}

impl trollm_mcp::ToolGateway for ControlledGateway {
    fn prepare_call(
        &mut self,
        call: &trollm_mcp::ToolCall,
        context: &trollm_mcp::preparation::PreparationContext,
    ) -> Result<Option<trollm_mcp::preparation::PreparedCall>> {
        self.ctl.before_boundary()?;
        let prepared = self.inner.prepare_call(call, context)?;
        self.ctl.before_boundary()?;
        Ok(prepared)
    }
    fn after_retained_intent(
        &mut self,
        call: &trollm_mcp::ToolCall,
        context: &trollm_mcp::preparation::PreparationContext,
        intent_seq: Option<u64>,
    ) -> Result<()> {
        self.ctl.before_boundary()?;
        self.inner
            .after_retained_intent(call, context, intent_seq)?;
        self.ctl.before_boundary()
    }
    fn after_retained_result(
        &mut self,
        call: &trollm_mcp::ToolCall,
        context: &trollm_mcp::preparation::PreparationContext,
        result: &trollm_mcp::ToolResult,
        intent_seq: Option<u64>,
        result_seq: u64,
    ) -> Result<()> {
        // Preserve a returned result before honoring cancellation; this callback
        // never grants another effect or acknowledges worker inactivity.
        self.inner
            .after_retained_result(call, context, result, intent_seq, result_seq)
    }
    fn set_call_context(&mut self, context: Option<trollm_mcp::CallContext>) {
        self.inner.set_call_context(context);
    }
    fn set_deadline(&mut self, deadline: Option<Instant>) {
        self.inner.set_deadline(deadline);
    }
    fn prepared_argument_names(&self, server: &str, tool: &str) -> Vec<String> {
        self.inner.prepared_argument_names(server, tool)
    }
    fn argument_constraints(&self, server: &str, tool: &str) -> Option<Value> {
        self.inner.argument_constraints(server, tool)
    }
    fn available_tools(&self, grants: &[String]) -> Vec<trollm_mcp::ToolDescriptor> {
        self.inner.available_tools(grants)
    }

    fn tool_class(&self, server: &str, tool: &str) -> trollm_mcp::policy::ToolClass {
        self.inner.tool_class(server, tool)
    }

    fn list_tools(&self) -> Result<Vec<trollm_mcp::ToolDescriptor>> {
        self.inner.list_tools()
    }

    fn validate_call(&self, call: &trollm_mcp::ToolCall) -> Result<()> {
        self.ctl.before_boundary()?;
        self.inner.validate_call(call)
    }

    fn call(&mut self, call: &trollm_mcp::ToolCall) -> Result<trollm_mcp::ToolResult> {
        self.ctl.before_boundary()?;
        self.inner.call(call)
    }

    fn begin_result_artifact_attempt(
        &mut self,
        server: &str,
        tool: &str,
        name: &str,
    ) -> Result<()> {
        self.ctl.before_boundary()?;
        self.inner.begin_result_artifact_attempt(server, tool, name)
    }

    fn call_with_result_artifact(
        &mut self,
        call: &trollm_mcp::ToolCall,
        name: Option<&str>,
    ) -> Result<trollm_mcp::ToolResult> {
        self.ctl.before_boundary()?;
        self.inner.call_with_result_artifact(call, name)
    }
}

/// Commands delivered to a worker parked at a plan-driven pause.
enum RunCommand {
    Resume,
    Cancel,
}

pub struct RunHandle {
    pub status: Arc<Mutex<RunPhase>>,
    pub control: Arc<RunControl>,
    cmd: Mutex<Option<Sender<RunCommand>>>,
    pub run_dir: PathBuf,
}

impl RunHandle {
    fn send(&self, command: RunCommand) {
        if let Some(tx) = self.cmd.lock().unwrap().as_ref() {
            let _ = tx.send(command);
        }
    }

    pub fn pause(&self) {
        // Plan-paused runs are already stopped; running runs get the
        // cooperative hold.
        self.control.request_pause();
    }

    pub fn resume(&self) {
        let mut gate = self.control.gate.lock().unwrap();
        if self.control.is_cancelled() {
            return;
        }
        self.control.pause_requested.store(false, Ordering::SeqCst);
        self.control.gate_cond.notify_all();
        // A cooperative hold (or a click while running) must not enqueue
        // permission to resume some future authored pause. Consume the
        // current parked state once, so duplicate requests cannot buffer.
        if gate.plan_paused {
            gate.plan_paused = false;
            *self.status.lock().unwrap() = RunPhase::Running;
            self.send(RunCommand::Resume);
        } else {
            let mut status = self.status.lock().unwrap();
            if matches!(*status, RunPhase::Paused { .. }) {
                *status = RunPhase::Running;
            }
        }
    }

    pub fn cancel(&self) {
        self.control.request_cancel();
        self.send(RunCommand::Cancel);
    }
}

/// Everything needed to (re)build an executor for a run.
struct RunPlan {
    plan: ExecutionPlan,
    registry: TaskRegistry,
    configuration: Option<RunConfiguration>,
    argument_constraints: std::collections::BTreeMap<String, Value>,
    selected_preparer: Option<Arc<crate::collection_provider::InstalledCollectionPreparer>>,
}

/// Closed first declared-text execution shape. Other declared sources require
/// their own explicit native mapping, never a relaxed generic source flag.
fn validate_declared_text(plan: &RunPlan) -> Result<()> {
    use trollm_core::plan::{NodeKind, RunLimitPolicy, Transition};
    use trollm_core::registry::TaskCompletion;
    let root = &plan.plan;
    if root.source_baseline.is_some()
        || plan.registry.source_baseline.is_some()
        || !root.subflows.is_empty()
        || root.models.len() != 1
        || !root.models.contains_key("coder")
        || !root.mcp.servers.is_empty()
        || !root.mcp.policies.is_empty()
        || root.limits.max_steps != 1
        || root.limits.max_reasoner_calls != 0
        || root.limits.max_tokens_total == 0
        || root.limits.timeout_s == 0
        || root.limits.artifact_max_mb == 0
        || root.on_limit != RunLimitPolicy::Abort
        || root.nodes.len() != 2
        || plan.registry.tasks.len() != 1
    {
        bail!("declared text profile requires one bounded no-tool generation");
    }
    let entry = root
        .nodes
        .get(&root.entry_step)
        .context("declared entry unavailable")?;
    if !matches!(entry.kind, NodeKind::Task)
        || entry.routes.is_some()
        || entry.on_error.is_some()
        || entry.verify.is_some()
        || entry.outputs.is_some()
    {
        bail!("declared text task routing unsupported");
    }
    let task = plan
        .registry
        .tasks
        .get(entry.task_ref.as_deref().unwrap_or(""))
        .context("declared text task unavailable")?;
    if task.model != "coder"
        || !task.tools.is_empty()
        || task.completion != TaskCompletion::ModelProse
        || task.loop_spec.is_some()
        || task.mutations.is_some()
        || task.tool_arguments.is_some()
        || task.result_artifact.is_some()
    {
        bail!("declared text task cannot loop, retry or use tools");
    }
    let Some(Transition::Node(next)) = &entry.on_verified else {
        bail!("declared text requires one terminal transition");
    };
    let end = root
        .nodes
        .get(next)
        .context("declared text end unavailable")?;
    if !matches!(end.kind, NodeKind::End) || end.outcome.as_deref() != Some("succeeded") {
        bail!("declared text terminal outcome unsupported");
    }
    Ok(())
}

fn load_run_plan(base: &Path, flow_sha: &str, installation: Option<&str>) -> Result<RunPlan> {
    let dir = match installation {
        Some(id) => crate::bundle::revision_dir(base, id)?,
        None => base.join("bundles").join(flow_sha),
    };
    let manifest_bytes = fs::read(dir.join(MANIFEST_NAME)).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            anyhow!(TrollmError::NotFound(format!(
                "no installed flow bundle for sha256 {flow_sha}: {error}"
            )))
        } else {
            anyhow!(error).context(format!("no installed flow bundle for sha256 {flow_sha}"))
        }
    })?;
    let manifest: BundleManifest = serde_json::from_slice(&manifest_bytes)
        .map_err(|error| TrollmError::Bundle(format!("{error}")))?;
    let (plan, registry) = crate::bundle::validate_materialized_bundle(&dir, &manifest)
        .map_err(|error| TrollmError::Bundle(format!("{error:#}")))?;
    if plan.sha256 != flow_sha {
        bail!(TrollmError::Bundle(
            "installed entry flow sha256 mismatch (corrupt bundles dir)".into()
        ));
    }
    Ok(RunPlan {
        plan,
        registry,
        configuration: None,
        argument_constraints: Default::default(),
        selected_preparer: None,
    })
}

// Keep scan and append limits identical so every record we write can reopen.
const MAX_PROXY_RECORD_BYTES: usize = 1024 * 1024;

/// Validate history before appending; never discard corrupt or partial records.
fn proxy_sink(events_path: PathBuf, run_id: String) -> Result<EventSink> {
    use std::io::{BufRead, BufReader, Write};
    let mut file = fs::OpenOptions::new()
        .create(true)
        .read(true)
        .append(true)
        .open(events_path)
        .context("open required proxy audit log")?;
    let mut seq = 0;
    let mut needs_delimiter = false;
    {
        let mut reader = BufReader::new(&file);
        loop {
            let mut record = Vec::new();
            let bytes = reader
                .by_ref()
                .take((MAX_PROXY_RECORD_BYTES + 1) as u64)
                .read_until(b'\n', &mut record)
                .context("read required proxy audit log")?;
            if bytes == 0 {
                break;
            }
            if bytes > MAX_PROXY_RECORD_BYTES
                || (bytes == MAX_PROXY_RECORD_BYTES && record.last() != Some(&b'\n'))
            {
                bail!("proxy audit record exceeds byte limit");
            }
            let envelope: Value = serde_json::from_slice(&record)
                .map_err(|_| anyhow!("invalid proxy audit record"))?;
            let next = envelope
                .get("seq")
                .and_then(Value::as_u64)
                .filter(|next| *next > seq)
                .context("proxy audit sequence must be positive and increasing")?;
            if envelope.get("runId").and_then(Value::as_str) != Some(run_id.as_str())
                || envelope.get("ts").and_then(Value::as_u64).is_none()
                || serde_json::from_value::<ProxyEvent>(envelope).is_err()
            {
                bail!("invalid proxy audit envelope");
            }
            seq = next;
            needs_delimiter = record.last() != Some(&b'\n');
        }
    }
    seq.checked_add(1)
        .context("proxy audit sequence exhausted")?;
    if needs_delimiter {
        file.write_all(b"\n")
            .context("delimit required proxy audit log")?;
        file.flush().context("flush required proxy audit log")?;
    }
    Ok(proxy_writer_sink(file, run_id, seq))
}

fn proxy_writer_sink(
    mut writer: impl std::io::Write + Send + 'static,
    run_id: String,
    mut seq: u64,
) -> EventSink {
    let mut failed = false;
    Box::new(move |event: ProxyEvent| {
        if failed {
            bail!("proxy audit writer previously failed");
        }
        let result = (|| -> Result<()> {
            let next = seq
                .checked_add(1)
                .context("proxy audit sequence exhausted")?;
            let mut obj = serde_json::to_value(&event)?;
            let fields = obj.as_object_mut().context("invalid proxy audit event")?;
            fields.insert("ts".into(), json!(trollm_core::events::now_ms()));
            fields.insert("runId".into(), json!(run_id));
            fields.insert("seq".into(), json!(next));
            let mut record = serde_json::to_vec(&obj)?;
            record.push(b'\n');
            if record.len() > MAX_PROXY_RECORD_BYTES {
                bail!("proxy audit record exceeds byte limit");
            }
            writer
                .write_all(&record)
                .context("write required proxy audit log")?;
            writer.flush().context("flush required proxy audit log")?;
            seq = next;
            Ok(())
        })();
        failed = result.is_err();
        result
    })
}

/// Operator-provisioned app trust, outside flow bundles and model-controlled data.
/// Absence preserves the transport's public roots; a broken configured file must
/// fail explicitly instead of silently falling back to a different trust policy.
fn load_mcp_roots(base: &Path) -> Result<RootCertificates> {
    const MAX_PEM_BYTES: u64 = 128 * 1024;
    let path = base.join("trust/mcp-roots.pem");
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(RootCertificates::default());
        }
        Err(error) => return Err(error).context("inspecting app MCP root certificates"),
    };
    if !metadata.is_file() {
        bail!("app MCP root certificates must be a regular PEM file");
    }
    let file = fs::File::open(&path).context("opening app MCP root certificates")?;
    let mut pem = Vec::new();
    file.take(MAX_PEM_BYTES + 1)
        .read_to_end(&mut pem)
        .context("reading app MCP root certificates")?;
    if pem.len() as u64 > MAX_PEM_BYTES {
        bail!("app MCP root certificates exceed 128 KiB");
    }
    RootCertificates::from_pem(&pem).context("invalid app MCP root certificates")
}

fn build_gateway(
    run_plan: &RunPlan,
    base: &Path,
    run_dir: &RunDir,
    token_provider: Option<SharedTokenProvider>,
    resume_existing: bool,
) -> Result<McpGateway> {
    let roots = if run_plan.plan.mcp.servers.is_empty() {
        RootCertificates::default()
    } else {
        load_mcp_roots(base)?
    };
    let servers: Vec<_> = run_plan
        .plan
        .mcp
        .servers
        .iter()
        .map(|s| {
            ServerConfig::new(s.name.clone(), s.url.clone()).with_root_certificates(roots.clone())
        })
        .collect();
    let authentication = RunAuthentication::resolve(
        run_dir.path(),
        &servers,
        token_provider.as_ref(),
        resume_existing,
    )?;
    let servers = servers
        .into_iter()
        .map(|server| {
            if let Some(preparer) = &run_plan.selected_preparer {
                authentication.bind_selected_static(
                    server,
                    token_provider.as_ref(),
                    preparer.installed_key_sha256()?,
                )
            } else {
                authentication.bind(server, token_provider.as_ref())
            }
        })
        .collect::<Result<Vec<_>>>()?;
    let policies = PolicySet::from_yaml_value(&serde_yaml::to_value(&run_plan.plan.mcp.policies)?)?;
    let sink = proxy_sink(
        run_dir.path().join("proxy.events.jsonl"),
        run_dir.run_id().to_string(),
    )?;
    let gateway = McpGateway::new(
        servers,
        policies,
        base.join("cache").join("mcp"),
        run_dir.artifacts_dir(),
        Some(sink),
    )?
    .with_argument_constraints(run_plan.argument_constraints.clone())?;
    match &run_plan.selected_preparer {
        Some(preparer) => gateway.with_preparer(vec!["e2llm.act".into()], preparer.clone()),
        None => Ok(gateway),
    }
}

// ---------------------------------------------------------------------------
// Model wiring (feature-gated; §9: weights are pinned by the app, never the
// plan — the plan contributes only slot names and ctx sizes).

/// Verify `models/<slot>.gguf` against its `models/<slot>.gguf.sha256`
/// manifest (§8). sha256sum-style lines ("<hex>  <name>") and bare hex are
/// both accepted.
#[cfg(any(feature = "llama", test))]
fn verify_model_weights(
    base: &Path,
    slot: &str,
    ctl: &RunControl,
    budget: &GenerationBudget,
) -> Result<PathBuf> {
    let path = base.join("models").join(format!("{slot}.gguf"));
    let manifest_path = base.join("models").join(format!("{slot}.gguf.sha256"));
    let expected_raw = fs::read_to_string(&manifest_path).with_context(|| {
        format!(
            "no sha256 manifest for model slot '{slot}' ({})",
            manifest_path.display()
        )
    })?;
    let expected = expected_raw
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_string();
    verify_weights(&path, &expected, slot, ctl, budget)?;
    Ok(path)
}

#[cfg(any(feature = "llama", test))]
fn verify_weights(
    path: &Path,
    expected: &str,
    slot: &str,
    ctl: &RunControl,
    budget: &GenerationBudget,
) -> Result<()> {
    verify_weights_with_checkpoint(path, expected, slot, &|| {
        ctl.before_boundary()?;
        budget.check()
    })
}

#[cfg(any(feature = "llama", test))]
fn verify_weights_with_checkpoint(
    path: &Path,
    expected: &str,
    slot: &str,
    checkpoint: &dyn Fn() -> Result<()>,
) -> Result<()> {
    let mut file = fs::File::open(path)
        .with_context(|| format!("no weights for model slot '{slot}' ({})", path.display()))?;
    use sha2::Digest;
    let mut h = sha2::Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        checkpoint()?;
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        h.update(&buffer[..count]);
    }
    let actual = format!("{:x}", h.finalize());
    if !actual.eq_ignore_ascii_case(expected) {
        bail!(
            "model weights sha256 mismatch for slot '{slot}': manifest {expected}, actual {actual}"
        );
    }
    Ok(())
}

/// Each executor owns its lazy slots. The injected loader runs only after
/// streaming verification, and a successfully loaded model is reused in this run.
#[cfg(any(feature = "llama", test))]
type ModelLoader = Box<dyn FnMut(&Path) -> Result<Box<dyn ModelSlot>> + Send>;

#[cfg(any(feature = "llama", test))]
struct LazyModel {
    base: PathBuf,
    slot: String,
    weights: Option<(PathBuf, String)>,
    loader: ModelLoader,
    loaded: Option<Box<dyn ModelSlot>>,
    ctl: Arc<RunControl>,
}

#[cfg(any(feature = "llama", test))]
impl ModelSlot for LazyModel {
    fn generate(&mut self, mut req: GenerateRequest) -> Result<GenerateResponse> {
        let mut loading_diagnostics = trollm_models::GenerationDiagnostics::default();
        let result = (|| -> Result<GenerateResponse> {
            let budget = GenerationBudget::new(req.max_duration);
            let _activity = ActivityGuard(self.ctl.clone());
            self.ctl.before_boundary()?;
            if budget.expired() {
                return Ok(generation_timeout());
            }
            if self.loaded.is_none() {
                self.ctl.set_activity(Some(RunActivityApi::VerifyingModel {
                    slot: self.slot.clone(),
                }));
                let verify_started = Instant::now();
                let verification = match &self.weights {
                    Some((path, expected)) => {
                        verify_weights(path, expected, &self.slot, &self.ctl, &budget)
                            .map(|()| path.clone())
                    }
                    None => verify_model_weights(&self.base, &self.slot, &self.ctl, &budget),
                };
                loading_diagnostics.weight_verify_ms =
                    Some(verify_started.elapsed().as_millis() as u64);
                let path = match verification {
                    Ok(path) => path,
                    Err(error) if error.is::<GenerationExpired>() => {
                        return Ok(generation_timeout())
                    }
                    Err(error) => return Err(error),
                };
                self.ctl.before_boundary()?;
                if budget.expired() {
                    return Ok(generation_timeout());
                }
                self.ctl.set_activity(Some(RunActivityApi::LoadingModel {
                    slot: self.slot.clone(),
                }));
                let load_started = Instant::now();
                let loaded = (self.loader)(&path);
                loading_diagnostics.load_ms = Some(load_started.elapsed().as_millis() as u64);
                self.loaded = Some(loaded?);
            }
            self.ctl.before_boundary()?;
            if budget.expired() {
                return Ok(generation_timeout());
            }
            self.ctl.set_activity(Some(RunActivityApi::Generating {
                slot: self.slot.clone(),
            }));
            req.max_duration = budget.remaining();
            self.loaded.as_mut().unwrap().generate(req)
        })();
        match result {
            Ok(mut response) => {
                response
                    .diagnostics
                    .get_or_insert_default()
                    .include_loading(&loading_diagnostics);
                Ok(response)
            }
            Err(error) => {
                let mut diagnostics = error
                    .downcast_ref::<trollm_models::ModelDiagnosticError>()
                    .map(|error| error.diagnostics.clone())
                    .unwrap_or_default();
                diagnostics.include_loading(&loading_diagnostics);
                Err(trollm_models::ModelDiagnosticError::attach(
                    error,
                    diagnostics,
                ))
            }
        }
    }
}

/// Collect every slot name + ctx declared by the plan tree.
#[cfg(any(feature = "llama", feature = "stub"))]
fn slot_decls(plan: &ExecutionPlan, out: &mut Vec<(String, u32)>) {
    for (name, decl) in &plan.models {
        out.push((name.clone(), decl.ctx));
    }
    for sub in plan.subflows.values() {
        slot_decls(sub, out);
    }
}

#[cfg(feature = "llama")]
fn build_slots(
    plan: &ExecutionPlan,
    registry: &TaskRegistry,
    base: &Path,
    configuration: Option<&RunConfiguration>,
    ctl: Arc<RunControl>,
) -> Result<(SlotMap<ControlledModel>, Preparations)> {
    let mut decls = Vec::new();
    slot_decls(plan, &mut decls);
    let mut map = SlotMap::new();
    let mut preparations = Preparations::default();
    let selected = preparation::selected_slots(plan, registry);
    for (name, ctx) in decls {
        if map.contains(&name) {
            continue;
        }
        let (weights, options) = match configuration {
            Some(configuration) => {
                let model = &configuration.models[&name];
                (
                    Some((PathBuf::from(&model.path), model.sha256.clone())),
                    trollm_models::llama::LlamaCppOptions {
                        temperature: model.temperature as f32,
                        top_p: model.top_p as f32,
                        ..Default::default()
                    },
                )
            }
            None => (None, Default::default()),
        };
        let lazy = LazyModel {
            base: base.to_path_buf(),
            slot: name.clone(),
            weights,
            loader: Box::new(move |path| {
                Ok(Box::new(trollm_models::llama::LlamaCppSlot::with_options(
                    path, ctx, options,
                )?))
            }),
            loaded: None,
            ctl: ctl.clone(),
        };
        let inner: Box<dyn ModelSlot> = if selected.contains(&name) {
            preparations.add(lazy)
        } else {
            Box::new(lazy)
        };
        map.insert(
            name.clone(),
            ControlledModel {
                slot: name,
                inner,
                ctl: ctl.clone(),
            },
        );
    }
    Ok((map, preparations))
}

#[cfg(all(feature = "stub", not(feature = "llama")))]
fn build_slots(
    plan: &ExecutionPlan,
    _registry: &TaskRegistry,
    _base: &Path,
    _configuration: Option<&RunConfiguration>,
    ctl: Arc<RunControl>,
) -> Result<(SlotMap<ControlledModel>, Preparations)> {
    let mut decls = Vec::new();
    slot_decls(plan, &mut decls);
    let mut map = SlotMap::new();
    for (name, _ctx) in decls {
        map.insert(
            name.clone(),
            ControlledModel {
                slot: name,
                inner: Box::new(StubModel),
                ctl: ctl.clone(),
            },
        );
    }
    Ok((map, Preparations::default()))
}

#[cfg(not(any(feature = "llama", feature = "stub")))]
fn build_slots(
    _plan: &ExecutionPlan,
    _registry: &TaskRegistry,
    _base: &Path,
    _configuration: Option<&RunConfiguration>,
    _ctl: Arc<RunControl>,
) -> Result<(SlotMap<ControlledModel>, Preparations)> {
    bail!(
        "this build has no model runtime: built without the 'llama' feature \
         (and without the 'stub' test feature)"
    )
}

/// Deterministic in-process model for tests and UI development builds
/// without weights: always terminates the coder loop with prose.
#[cfg(all(feature = "stub", not(feature = "llama")))]
struct StubModel;

#[cfg(all(feature = "stub", not(feature = "llama")))]
impl ModelSlot for StubModel {
    fn generate(&mut self, req: GenerateRequest) -> Result<GenerateResponse> {
        let text = "stub model response".to_string();
        Ok(GenerateResponse {
            tokens_in: req.prompt.split_whitespace().count() as u32,
            tokens_out: 4,
            text,
            finish: trollm_models::FinishReason::Stop,
            capacity: None,
            diagnostics: None,
        })
    }
}

// ---------------------------------------------------------------------------
// Worker

pub struct SpawnedRun {
    pub handle: Arc<RunHandle>,
    pub run_id: String,
}

/// Spawn a worker thread for a new run (fresh RunDir) or, when
/// `resume_run_id` is set, over the existing run dir (core's resume/crash
/// path: `Executor::new` restores from `state.snapshot.json`).
pub fn spawn_run(
    base: PathBuf,
    flow_sha: String,
    user_input: String,
    token_provider: Option<SharedTokenProvider>,
    resume_run_id: Option<String>,
    configuration: Option<RunConfiguration>,
) -> Result<SpawnedRun> {
    spawn_source_run(
        base,
        flow_sha,
        user_input,
        token_provider,
        resume_run_id,
        configuration,
        None,
    )
}

#[cfg_attr(
    not(any(feature = "llama", feature = "stub")),
    allow(unused_variables, unreachable_code)
)]
pub fn spawn_source_run(
    base: PathBuf,
    flow_sha: String,
    user_input: String,
    token_provider: Option<SharedTokenProvider>,
    resume_run_id: Option<String>,
    configuration: Option<RunConfiguration>,
    source: Option<(String, Value)>,
) -> Result<SpawnedRun> {
    spawn_installed_source_run(
        base,
        flow_sha,
        user_input,
        token_provider,
        resume_run_id,
        configuration,
        source,
        None,
    )
}

/// Native embedding entry. The ordinary/public source entry cannot infer or
/// manufacture this out-of-band installation from a publication support flag.
#[allow(clippy::too_many_arguments)]
#[cfg_attr(
    not(any(feature = "llama", feature = "stub")),
    allow(unused_variables, unreachable_code)
)]
pub fn spawn_installed_source_run(
    base: PathBuf,
    flow_sha: String,
    user_input: String,
    token_provider: Option<SharedTokenProvider>,
    resume_run_id: Option<String>,
    configuration: Option<RunConfiguration>,
    source: Option<(String, Value)>,
    selected_installation: Option<crate::collection_provider::SelectedRunInstallation>,
) -> Result<SpawnedRun> {
    // Fail synchronously when this build carries no model runtime at all.
    #[cfg(not(any(feature = "llama", feature = "stub")))]
    bail!(TrollmError::ModelUnavailable(
        "this build has no model runtime: built without the 'llama' feature (and without the 'stub' test feature)".into()
    ));

    let (installation, source_association) = match source {
        Some((id, association)) => (Some(id), Some(association)),
        None => (None, None),
    };
    // A resumed run uses only its original installation sidecar. Never select
    // the latest dependency revision, even when the entry flow is unchanged.
    let installation = match &resume_run_id {
        Some(id) => match fs::read(base.join("runs").join(id).join("installation.json")) {
            Ok(bytes) => Some(serde_json::from_slice::<String>(&bytes)?),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if base
                    .join("runs")
                    .join(id)
                    .join("source-association.json")
                    .exists()
                {
                    bail!("source run is missing its pinned installation identity");
                }
                None
            }
            Err(error) => return Err(error.into()),
        },
        None => installation,
    };
    let mut source_association = match &resume_run_id {
        Some(id) if installation.is_some() => Some(serde_json::from_slice::<Value>(&fs::read(
            base.join("runs").join(id).join("source-association.json"),
        )?)?),
        _ => source_association,
    };
    if let Some(id) = &installation {
        let source = source_association
            .as_ref()
            .context("source run requires retained installation association")?;
        let installed = &source["installation"];
        let identity = &source["descriptor"]["identity"];
        if installed["installationSha256"].as_str() != Some(id.as_str())
            || installed["flowSha256"].as_str() != Some(flow_sha.as_str())
            || identity["manifestSha256"].as_str() != Some(id.as_str())
            || identity["entryFlowSha256"].as_str() != Some(flow_sha.as_str())
            || installed["archiveSha256"].as_str().is_none()
            || installed["archiveSha256"] != identity["sha256"]
        {
            bail!("source installation association does not match selected native revision");
        }
    }
    let mut run_plan = load_run_plan(&base, &flow_sha, installation.as_deref())?;
    let setup = match &installation {
        Some(id) => crate::bundle::revision_setup(&base, id)?,
        None => crate::bundle::installed_flow_setup(&base, &flow_sha)?,
    };
    let declared = source_association
        .as_ref()
        .is_some_and(|source| source["descriptor"]["source"]["origin"] == "DECLARED");
    let mut inspected_collection = None;
    if declared {
        let source = &source_association.as_ref().unwrap()["descriptor"]["source"];
        match source["schemaVersion"].as_str() {
            Some(crate::collection_members::SOURCE_SCHEMA) => {
                let source: crate::collection_members::CollectionSource =
                    serde_json::from_value(source.clone())?;
                let id = installation
                    .as_deref()
                    .context("collection source requires immutable installation")?;
                if source.installation_sha256 != id {
                    bail!("collection source installation association drift");
                }
                let servers = &run_plan.plan.mcp.servers;
                if servers.len() != 1 || servers[0].name != "e2llm" {
                    bail!("collection source requires exact selected MCP server");
                }
                let inspected =
                    source.inspect(&crate::bundle::revision_dir(&base, id)?, &servers[0].url)?;
                if inspected.root_plan_pin
                    != format!("{}@sha256:{}", run_plan.plan.id, run_plan.plan.sha256)
                {
                    bail!("collection source selected root drift");
                }
                inspected_collection = Some(inspected);
            }
            Some("moray.declared-native-text.v1") => validate_declared_text(&run_plan)?,
            _ => bail!("declared source schema unsupported"),
        }
        if inspected_collection.is_none() {
            let input = setup
                .as_ref()
                .and_then(|setup| setup.input.as_ref())
                .context("declared text input unavailable")?;
            let fields = input
                .fields
                .as_ref()
                .context("declared text input unavailable")?;
            if fields.len() != 1
                || !fields[0].required
                || !matches!(fields[0].kind, crate::setup::InputKind::Text)
                || !input.tool_arguments.is_empty()
                || !setup.as_ref().unwrap().connections.is_empty()
            {
                bail!("declared text requires one required text field and no connections");
            }
        }
    }
    let supplied_admission = source_association
        .as_ref()
        .and_then(|source| source.get("inputAdmission"));
    if declared && supplied_admission.is_none() {
        bail!("declared source requires native input admission");
    }
    let original_input = if let Some(id) = &resume_run_id {
        if supplied_admission.is_some() {
            // Continuation retains the original input, never resubmits edited UI values.
            String::from_utf8(crate::setup::read_input_evidence(
                &base.join("runs").join(id),
                "input.original.json",
                65536,
            )?)?
        } else {
            user_input.clone()
        }
    } else {
        user_input.clone()
    };
    let mut admitted_input = None;
    let user_input = match setup.and_then(|setup| setup.input) {
        Some(input) => {
            if let Some(supplied) = supplied_admission {
                let apk = supplied["apkNormalized"]
                    .as_str()
                    .context("missing APK input agreement")?;
                let admitted = crate::setup::admit_source_input(&input, &original_input, apk)?;
                if &admitted != supplied {
                    bail!("source input admission identity mismatch");
                }
                if let Some(id) = &resume_run_id {
                    let directory = base.join("runs").join(id);
                    let saved: Value = serde_json::from_slice(&crate::setup::read_input_evidence(
                        &directory,
                        "input-admission.json",
                        524288,
                    )?)?;
                    if saved != admitted
                        || crate::setup::read_input_evidence(
                            &directory,
                            "input.normalized.json",
                            65536,
                        )? != apk.as_bytes()
                    {
                        bail!("retained input admission changed");
                    }
                }
                if resume_run_id.is_some() && user_input != apk {
                    bail!("source continuation input changed");
                }
                admitted_input = Some(admitted);
            }
            let resolved = input.resolve(&original_input)?;
            run_plan.argument_constraints = resolved.argument_constraints;
            resolved.canonical
        }
        None if supplied_admission.is_some() || declared => {
            bail!("structured source input unavailable")
        }
        None => user_input,
    };
    if let Some(inspected) = &inspected_collection {
        if user_input != inspected.selection_input_json {
            bail!("collection admitted input differs from immutable selected roster");
        }
        // Exact input/member agreement is not execution authority, including
        // empty/complete-prefix cases. No run directory or worker exists yet.
        if selected_installation.is_none() {
            bail!("collection installed authority/preparer/worker binding unavailable");
        }
        if resume_run_id.is_some() {
            bail!("selected cold reconstruction requires original actor/custody recovery; unavailable");
        }
    } else if selected_installation.is_some() {
        bail!("selected installation requires inspected collection source");
    }
    let run_dir = match &resume_run_id {
        Some(id) => {
            let dir = RunDir::new(base.join("runs").join(id));
            if !dir.snapshot_path().exists() {
                bail!(TrollmError::NotFound(format!(
                    "no resumable snapshot for run {id}"
                )));
            }
            dir
        }
        None => RunDir::under(&base),
    };
    let run_id = run_dir.run_id().to_string();
    if let Some(source) = &mut source_association {
        if resume_run_id.is_some() {
            if source["nativeRunId"].as_str() != Some(run_id.as_str()) {
                bail!("source association belongs to a different native run");
            }
        } else {
            source["nativeRunId"] = Value::String(run_id.clone());
            // One native attempt spans explicit cold continuation of this run.
            // Reporting retries do not allocate a new run or attempt.
            source["nativeAttemptId"] = Value::String(format!("{run_id}-source-1"));
        }
    }

    // Resume always uses recorded effective settings; UI/default changes cannot
    // silently select different weights or endpoints for an existing run.
    let mut configuration = if resume_run_id.is_some() {
        RunConfiguration::load(run_dir.path())?
    } else {
        configuration
    };
    if let Some(configuration) = &mut configuration {
        configuration.validate_and_resolve(&run_plan.plan)?;
        if inspected_collection.is_some() {
            crate::collection_provider::SelectedRunInstallation::validate_configuration(
                configuration,
                &run_plan.plan.mcp.servers[0].url,
            )?;
        }
        configuration.apply_to_plan(&mut run_plan.plan);
    }

    let retain_initial_inputs = || -> Result<()> {
        run_dir.create_dirs()?;
        if let Some(source) = &source_association {
            trollm_core::artifacts::write_atomic(
                run_dir.path(),
                "source-association.json",
                &serde_json::to_vec(source)?,
            )?;
        }
        if let Some(id) = &installation {
            trollm_core::artifacts::write_atomic(
                run_dir.path(),
                "installation.json",
                &serde_json::to_vec(id)?,
            )?;
        }
        if let Some(admission) = &admitted_input {
            for (name, bytes) in [
                ("input.original.json", original_input.as_bytes().to_vec()),
                ("input.normalized.json", user_input.as_bytes().to_vec()),
                ("input-admission.json", serde_json::to_vec(admission)?),
            ] {
                trollm_core::artifacts::write_atomic(run_dir.path(), name, &bytes)?;
            }
        }
        fs::write(run_dir.path().join("user_input.txt"), &user_input)?;
        if let Some(configuration) = &configuration {
            configuration.save(run_dir.path())?;
        }
        Ok(())
    };

    let selected_start = selected_installation.is_some();
    if let Some(installation) = selected_installation {
        if configuration.is_none() {
            bail!("selected source requires explicit effective runtime configuration");
        }
        let association = source_association
            .as_ref()
            .context("selected source association unavailable")?;
        // Allocate the genuine native run before binding. Failed installation
        // can retain an unstarted directory/claim but never starts a worker.
        crate::selected_start::at(
            retain_initial_inputs(),
            "SELECTED_INITIAL_INPUT_CUSTODY",
            run_dir.path(),
        )?;
        run_plan.selected_preparer = Some(
            installation.bind(
                &base,
                run_dir.path(),
                association,
                inspected_collection
                    .as_ref()
                    .context("selected member inspection unavailable")?,
                token_provider
                    .as_ref()
                    .context("selected app provider unavailable")?
                    .clone(),
                &run_plan.plan.mcp.servers[0].url,
            )?,
        );
    }

    let observation =
        crate::observation::Projection::seed(&base, &run_id).unwrap_or_else(|error| {
            // Core still performs its own authoritative reconstruction and publishes
            // its usual failure. A disposable observer cannot replace that path.
            let mut view = crate::observation::Projection::default();
            view.report_error = Some(format!("observation seed failed: {error:#}"));
            view
        });
    let status = Arc::new(Mutex::new(RunPhase::Running));
    let control = Arc::new(RunControl {
        hold_ns: AtomicU64::new(0),
        observation: Arc::new(Mutex::new(observation)),
        cancel: AtomicBool::new(false),
        cancellation_cause: Mutex::new(None),
        worker_active: AtomicBool::new(true),
        selected_preparer: run_plan.selected_preparer.clone(),
        volatile_worker_failure: AtomicBool::new(false),
        activity: Mutex::new(Some(RunActivityApi::Starting)),
        preparation_activity: Mutex::new(HashMap::new()),
        pause_requested: AtomicBool::new(false),
        gate: Mutex::new(PauseGate::default()),
        gate_cond: Condvar::new(),
        status: status.clone(),
    });
    let (tx, rx) = channel();

    let handle = Arc::new(RunHandle {
        status,
        control: control.clone(),
        cmd: Mutex::new(Some(tx)),
        run_dir: run_dir.path().to_path_buf(),
    });

    if resume_run_id.is_none() {
        if !selected_start {
            retain_initial_inputs()?;
        }
        // Public event polling must recognize the returned run while model
        // initialization is still pending. The executor appends real events
        // later; fresh-run creation must never truncate an existing log.
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(run_dir.events_path())
            .context("creating run event log")?;
    }

    let thread_control = control.clone();
    let resume_existing = resume_run_id.is_some();
    run_plan.configuration = configuration;
    let failure_dir = run_dir.path().to_path_buf();
    std::thread::Builder::new()
        .name(format!("trollm-run-{run_id}"))
        .spawn(move || {
            let _worker = WorkerGuard(thread_control.clone());
            let result = drive(
                run_plan,
                base,
                run_dir,
                user_input,
                token_provider,
                (thread_control.clone(), rx),
                resume_existing,
            );
            if let Err(e) = result {
                publish_worker_failure(&failure_dir, &thread_control, e);
            }
            if retain_cancellation_cause(&failure_dir, &thread_control).is_err() {
                thread_control
                    .volatile_worker_failure
                    .store(true, Ordering::SeqCst);
            }
        })
        .context("spawning run worker")?;

    Ok(SpawnedRun { handle, run_id })
}

fn retain_cancellation_cause(directory: &Path, control: &RunControl) -> Result<()> {
    if let Some(cause) = *control
        .cancellation_cause
        .lock()
        .unwrap_or_else(|e| e.into_inner())
    {
        let record = json!({"schema":"trollm.native-cancellation-cause.v1-proposal",
            "nativeRunId":directory.file_name().and_then(|v| v.to_str()),
            "cause":cause,"meaning":"cooperative-stop-cause-only","effectDisposition":"unknown"});
        trollm_core::artifacts::write_atomic(
            directory,
            "cancellation-cause.json",
            &serde_json::to_vec(&record)?,
        )?;
    }
    Ok(())
}

fn publish_worker_failure(failure_dir: &Path, ctl: &RunControl, error: anyhow::Error) {
    let mut reason = format!("run worker error: {error:#}");
    let bytes =
        serde_json::to_vec(&json!({"reason": reason})).expect("string failure record serializes");
    if let Err(persist_error) =
        trollm_core::artifacts::write_atomic(failure_dir, WORKER_FAILURE_FILE, &bytes)
    {
        ctl.volatile_worker_failure.store(true, Ordering::SeqCst);
        reason.push_str(&format!(" (could not persist failure: {persist_error})"));
    }
    *ctl.status.lock().unwrap() = RunPhase::Failed { reason };
}

fn drive(
    run_plan: RunPlan,
    base: PathBuf,
    run_dir: RunDir,
    user_input: String,
    token_provider: Option<SharedTokenProvider>,
    controls: (Arc<RunControl>, Receiver<RunCommand>),
    resume_existing: bool,
) -> Result<()> {
    let (ctl, rx) = controls;
    if let Some(preparer) = &ctl.selected_preparer {
        preparer.start_clock_maintenance()?;
    }
    let configuration = run_plan.configuration.clone();
    // For new legacy native runs, capture the old app-private slots once too.
    #[cfg(feature = "llama")]
    let configuration = match configuration {
        None if !resume_existing => {
            let value = RunConfiguration::legacy(&run_plan.plan, &base)?;
            value.save(run_dir.path())?;
            Some(value)
        }
        other => other,
    };
    let gateway = build_gateway(&run_plan, &base, &run_dir, token_provider, resume_existing)?;
    let gateway = ControlledGateway {
        inner: gateway.with_send_gate({
            let control = ctl.clone();
            Arc::new(move || {
                if control.is_cancelled() {
                    bail!(CANCEL_MARKER);
                }
                Ok(())
            })
        }),
        ctl: ctl.clone(),
    };
    let (slots, mut preparations) = build_slots(
        &run_plan.plan,
        &run_plan.registry,
        &base,
        configuration.as_ref(),
        ctl.clone(),
    )?;
    let mut exec = Executor::new(
        run_plan.plan,
        run_plan.registry,
        user_input,
        slots,
        gateway,
        run_dir,
    )?;
    exec.set_control(ctl.clone());
    let observation = ctl.observation.clone();
    exec.set_event_sink(Arc::new(move |value, line| {
        if let Ok(mut view) = observation.lock() {
            view.committed(value, line);
        }
    }));
    // A repaired cold startup supersedes a previous worker failure. Preserve
    // that failure until reconstruction actually succeeds, then let the core
    // snapshot be authoritative again.
    match fs::remove_file(
        base.join("runs")
            .join(exec.run_id())
            .join(WORKER_FAILURE_FILE),
    ) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(anyhow!(error).context("clearing previous worker failure")),
    }

    // Reconstruction is complete. Execution may wait in authored backoff
    // without invoking a model, so Starting must not survive into that wait.
    ctl.set_activity(None);
    start_run_preparations(&mut preparations, &exec)?;

    // Cold resume carries its intent into worker startup; it must not depend
    // on a buffered command sent before the worker publishes its pause.
    let mut result = if resume_existing {
        exec.resume()
    } else {
        exec.run_until_blocked()
    };
    loop {
        let phase = match result {
            Ok(RunStatus::Running) => RunPhase::Running,
            Ok(RunStatus::Paused { reason }) => RunPhase::Paused { reason },
            Ok(RunStatus::Succeeded) => RunPhase::Succeeded,
            Ok(RunStatus::Failed { reason }) => RunPhase::Failed { reason },
            Err(error) => return Err(error),
        };
        let terminal = !matches!(phase, RunPhase::Running | RunPhase::Paused { .. });
        if terminal {
            preparations.stop_and_join();
        }
        // Resolve complete prose once, before publishing terminal success. The
        // compact source node survives eviction from the technical-event ring.
        if matches!(phase, RunPhase::Succeeded) {
            let source = ctl
                .observation
                .lock()
                .ok()
                .and_then(|view| view.report_node.clone());
            let report =
                final_report_for_node(&base.join("runs").join(exec.run_id()), source.as_deref());
            if let Ok(mut view) = ctl.observation.lock() {
                match report {
                    Ok(report) => view.final_report = report,
                    Err(error) => view.report_error = Some(format!("{error:#}")),
                }
            }
        }
        ctl.set_activity(None);
        {
            let mut gate = ctl.gate.lock().unwrap();
            gate.plan_paused = matches!(phase, RunPhase::Paused { .. });
            *ctl.status.lock().unwrap() = phase.clone();
        }
        if terminal {
            return Ok(());
        }
        if ctl.is_cancelled() {
            exec.try_cancel()?;
            preparations.stop_and_join();
            *ctl.status.lock().unwrap() = RunPhase::Failed {
                reason: "cancelled".to_string(),
            };
            return Ok(());
        }
        // Plan-driven pause: park until resume/cancel.
        let command = if ctl.selected_preparer.is_some() {
            loop {
                if ctl.is_cancelled() {
                    break Some(RunCommand::Cancel);
                }
                match rx.recv_timeout(SELECTED_CONTROL_RECHECK) {
                    Ok(command) => break Some(command),
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break None,
                }
            }
        } else {
            rx.recv().ok()
        };
        match command {
            Some(RunCommand::Resume) => {
                result = exec.resume();
            }
            Some(RunCommand::Cancel) => {
                exec.try_cancel()?;
                preparations.stop_and_join();
                *ctl.status.lock().unwrap() = RunPhase::Failed {
                    reason: "cancelled".to_string(),
                };
                return Ok(());
            }
            None => return Ok(()), // runtime dropped; snapshot is persisted
        }
    }
}

fn start_run_preparations<M: ModelSlot, G: trollm_mcp::ToolGateway>(
    preparations: &mut Preparations,
    exec: &Executor<M, G>,
) -> Result<()> {
    preparations.start(exec.remaining_run_duration())
}

// ---------------------------------------------------------------------------
// Status / tail helpers (pure file reads — no executor involvement).

/// A completed snapshot cannot be resumed. Worker failures over unfinished
/// snapshots remain eligible for explicitly requested reconstruction.
pub(crate) fn terminal_snapshot(base: &Path, run_id: &str) -> Result<bool> {
    let path = base.join("runs").join(run_id).join("state.snapshot.json");
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.into()),
    };
    let state: trollm_core::RunState =
        serde_json::from_slice(&bytes).context("parsing state.snapshot.json")?;
    Ok(matches!(
        state.status,
        trollm_core::state::StateStatus::Succeeded | trollm_core::state::StateStatus::Failed
    ))
}

/// Best-effort status for runs unknown to the in-process registry (e.g.
/// after an app restart): read the persisted snapshot (§4.4).
pub fn status_from_disk(base: &Path, run_id: &str) -> Result<Option<RunPhase>> {
    let dir = base.join("runs").join(run_id);
    match fs::read(dir.join(WORKER_FAILURE_FILE)) {
        Ok(bytes) => {
            let failure: Value =
                serde_json::from_slice(&bytes).context("parsing persisted worker failure")?;
            let reason = failure
                .get("reason")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow!("persisted worker failure has no reason"))?;
            return Ok(Some(RunPhase::Failed {
                reason: reason.to_string(),
            }));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let snapshot = dir.join("state.snapshot.json");
    if !snapshot.exists() {
        if dir.join("events.jsonl").is_file() {
            return Ok(Some(RunPhase::Failed {
                reason: "interrupted before saved progress; start a new run".to_string(),
            }));
        }
        return Ok(None);
    }
    let state: trollm_core::RunState =
        serde_json::from_slice(&fs::read(&snapshot)?).context("parsing state.snapshot.json")?;
    Ok(Some(match state.status {
        trollm_core::state::StateStatus::Running => RunPhase::Running,
        trollm_core::state::StateStatus::Paused => RunPhase::Paused {
            reason: "paused (persisted)".to_string(),
        },
        trollm_core::state::StateStatus::Succeeded => RunPhase::Succeeded,
        trollm_core::state::StateStatus::Failed => RunPhase::Failed {
            reason: state
                .terminal_reason
                .unwrap_or_else(|| "failed (persisted)".to_string()),
        },
    }))
}

/// The plan sha256 a persisted run belongs to, for cold resume.
pub fn plan_sha_of_run(base: &Path, run_id: &str) -> Result<String> {
    let snapshot = base.join("runs").join(run_id).join("state.snapshot.json");
    let bytes = fs::read(&snapshot).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            anyhow!(TrollmError::NotFound(format!(
                "reading state.snapshot.json: {error}"
            )))
        } else {
            anyhow!(error)
        }
    })?;
    let state: trollm_core::RunState =
        serde_json::from_slice(&bytes).context("parsing state.snapshot.json")?;
    let pin = state
        .stack
        .first()
        .map(|f| f.plan.clone())
        .ok_or_else(|| anyhow!("snapshot has empty stack"))?;
    // Frame identity is `<planId>@sha256:<hex>` (§4.4).
    Ok(pin.split("@sha256:").nth(1).unwrap_or(&pin).to_string())
}

/// `(events, max_seq)` as returned by [`tail_events`]: each event is
/// `(seq, type, raw jsonl line)`.
pub type TailResult = (Vec<(u64, String, String)>, u64);

/// Read `events.jsonl`, returning events with `seq > from_seq` plus the
/// current maximum seq. Marshals only — no interpretation of payloads.
pub fn tail_events(base: &Path, run_id: &str, from_seq: u64) -> Result<TailResult> {
    let path = base.join("runs").join(run_id).join("events.jsonl");
    let mut out = Vec::new();
    let last = trollm_core::EventLog::visit(&path, Some(run_id), |event, line| {
        // The visitor validates every record, including the unreturned prefix.
        let seq = event["seq"].as_u64().expect("validated event sequence");
        if seq > from_seq {
            out.push((
                seq,
                event["type"]
                    .as_str()
                    .expect("validated event type")
                    .to_owned(),
                line.to_owned(),
            ));
        }
    })?;
    Ok((out, last.max(from_seq)))
}

/// Read the complete validated journal without retaining event payloads.
pub fn last_event_seq(base: &Path, run_id: &str) -> Result<u64> {
    Ok(trollm_core::EventLog::visit(
        &base.join("runs").join(run_id).join("events.jsonl"),
        Some(run_id),
        |_, _| {},
    )?)
}

/// Final report heuristic (see OPEN QUESTIONS in the implementation report):
/// §9 has no result accessor, so on `Succeeded` the report is recovered from
/// the run's own records — the last root-frame node to transition
/// `onVerified` into an `end` node (from the event log), whose bound prose
/// outputs are read back out of `state.snapshot.json`. The wrapper passes
/// prose through verbatim; it never parses it.
pub fn final_report(base: &Path, run_id: &str) -> Result<Option<String>> {
    let dir = base.join("runs").join(run_id);
    let mut last_done_node = None;
    trollm_core::EventLog::visit(&dir.join("events.jsonl"), Some(run_id), |event, _| {
        if event["type"] == EventType::Transition.as_str() && event["key"] == "onVerified" {
            if let Some(node) = event["from"].as_str() {
                last_done_node = Some(node.to_owned());
            }
        }
    })?;
    final_report_for_node(&dir, last_done_node.as_deref())
}

pub(crate) fn final_report_for_node(
    dir: &Path,
    last_done_node: Option<&str>,
) -> Result<Option<String>> {
    let Some(last_done_node) = last_done_node else {
        return Ok(None);
    };
    // Preserve the existing best-effort snapshot/report-absence behavior.
    let Some(state) = fs::read(dir.join("state.snapshot.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<trollm_core::RunState>(&bytes).ok())
    else {
        return Ok(None);
    };
    let Some(outputs) = state
        .stack
        .first()
        .and_then(|frame| frame.node_outputs.get(last_done_node))
    else {
        return Ok(None);
    };
    let mut parts: Vec<&str> = outputs.values().filter_map(Value::as_str).collect();
    parts.sort_unstable();
    Ok(if parts.is_empty() {
        None
    } else {
        Some(parts.join("\n"))
    })
}

/// Inactive handles are only a cache; persisted run data outlives eviction.
const INACTIVE_RUN_CACHE_LIMIT: usize = 32;

/// Registry of live workers and a bounded cache of inactive run handles.
#[derive(Default)]
pub struct RunRegistry {
    state: Mutex<RegistryState>,
}

#[derive(Default)]
struct RegistryState {
    runs: HashMap<String, Arc<RunHandle>>,
    registration_order: VecDeque<String>,
}

impl RegistryState {
    fn cache_eligible(handle: &RunHandle) -> bool {
        // Terminal phase publication precedes worker teardown. Releasing a
        // handle then would lose control of a worker that still owns resources.
        !handle.control.worker_active()
            && !handle
                .control
                .volatile_worker_failure
                .load(Ordering::SeqCst)
    }

    fn prune(&mut self) {
        let mut excess = self
            .runs
            .values()
            .filter(|handle| Self::cache_eligible(handle))
            .count()
            .saturating_sub(INACTIVE_RUN_CACHE_LIMIT);
        if excess == 0 {
            return;
        }
        self.registration_order.retain(|id| {
            let evict = excess > 0 && Self::cache_eligible(&self.runs[id]);
            if evict {
                self.runs.remove(id);
                excess -= 1;
            }
            !evict
        });
    }
}

impl RunRegistry {
    pub fn insert(&self, run_id: String, handle: Arc<RunHandle>) {
        let mut state = self.state.lock().unwrap();
        if state.runs.insert(run_id.clone(), handle).is_some() {
            // Reconstructed workers get a fresh registration age. Never leave
            // a duplicate ID that could later evict its replacement early.
            state.registration_order.retain(|id| id != &run_id);
        }
        state.registration_order.push_back(run_id);
        state.prune();
    }

    pub fn get(&self, run_id: &str) -> Option<Arc<RunHandle>> {
        let mut state = self.state.lock().unwrap();
        state.prune();
        state.runs.get(run_id).cloned()
    }

    pub fn has_active(&self) -> bool {
        let mut state = self.state.lock().unwrap();
        state.prune();
        state.runs.values().any(|handle| {
            handle.control.worker_active()
                && matches!(
                    *handle.status.lock().unwrap(),
                    RunPhase::Running | RunPhase::Paused { .. }
                )
        })
    }
}

#[cfg(test)]
#[allow(dead_code)]
#[path = "../../trollm-mcp/tests/common/mock.rs"]
mod gateway_fixture;

#[cfg(test)]
mod regression_tests {
    use super::*;
    use std::sync::mpsc::RecvTimeoutError;
    use std::time::{Duration, Instant};

    // Public, synthetic loopback certificates shared with the MCP HTTPS tests.
    #[allow(dead_code)]
    mod tls_fixture {
        include!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../trollm-mcp/tests/common/tls_fixture.rs"
        ));
    }

    struct TrustTestDir(PathBuf);

    impl TrustTestDir {
        fn new(name: &str) -> Self {
            let base =
                std::env::temp_dir().join(format!("trollm-trust-{name}-{}", std::process::id()));
            fs::create_dir_all(base.join("trust")).unwrap();
            Self(base)
        }
    }

    impl Drop for TrustTestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn journal_record(run_id: &str, seq: u64) -> String {
        format!("{{\"ts\":1,\"runId\":\"{run_id}\",\"seq\":{seq},\"type\":\"run_started\"}}")
    }

    #[test]
    fn tail_validates_prefix_and_preserves_raw_records_and_future_cursor() {
        let base = TrustTestDir::new("bounded-tail");
        let dir = base.0.join("runs/tail");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("events.jsonl");
        assert_eq!(tail_events(&base.0, "tail", 5).unwrap(), (vec![], 5));
        assert_eq!(last_event_seq(&base.0, "tail").unwrap(), 0);
        let first = journal_record("tail", 1);
        let second = journal_record("tail", 3);
        let bytes = format!("{first}\r\n  {second}  ");
        fs::write(&path, &bytes).unwrap();
        let (events, last) = tail_events(&base.0, "tail", 1).unwrap();
        assert_eq!(last, 3);
        assert_eq!(
            events,
            vec![(3, "run_started".into(), format!("  {second}  "))]
        );
        assert_eq!(tail_events(&base.0, "tail", 100).unwrap(), (vec![], 100));
        assert_eq!(last_event_seq(&base.0, "tail").unwrap(), 3);
        assert_eq!(fs::read(&path).unwrap(), bytes.as_bytes());

        for invalid in [
            format!("malformed\n{second}\n"),
            format!("{}\n{second}\n", journal_record("another-run", 1)),
            format!("{first}\n{first}\n{second}\n"),
        ] {
            fs::write(&path, &invalid).unwrap();
            assert!(tail_events(&base.0, "tail", 100).is_err());
            assert!(last_event_seq(&base.0, "tail").is_err());
            assert_eq!(fs::read(&path).unwrap(), invalid.as_bytes());
        }
    }

    #[test]
    fn tail_retries_partial_append_without_advancing_or_altering_the_journal() {
        use std::io::Write;
        let base = TrustTestDir::new("bounded-partial");
        let dir = base.0.join("runs/partial");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("events.jsonl");
        let first = journal_record("partial", 1);
        let second = journal_record("partial", 2);
        let split = second.len() / 2;
        let incomplete = format!("{first}\n{}", &second[..split]);
        fs::write(&path, &incomplete).unwrap();
        assert!(tail_events(&base.0, "partial", 1).is_err());
        assert_eq!(fs::read(&path).unwrap(), incomplete.as_bytes());
        let mut file = fs::OpenOptions::new().append(true).open(&path).unwrap();
        file.write_all(&second.as_bytes()[split..]).unwrap();
        file.write_all(b"\n").unwrap();
        let (events, last) = tail_events(&base.0, "partial", 1).unwrap();
        assert_eq!(last, 2);
        assert_eq!(events, vec![(2, "run_started".into(), second)]);
    }

    #[test]
    fn cached_observation_revalidates_only_after_invalidation_and_keeps_full_report() {
        let base = TrustTestDir::new("live-observation-cold");
        let rt = crate::TrollmRuntime::new(base.0.to_string_lossy().into_owned(), None).unwrap();
        let id = "00000000-0000-0000-0000-000000000078";
        let dir = rt.base.join("runs").join(id);
        fs::create_dir_all(&dir).unwrap();
        let mut state = trollm_core::RunState::fresh(id, "flow@sha256:absent", "report");
        state.status = trollm_core::state::StateStatus::Succeeded;
        let prose = "complete report\n".repeat(40_000);
        state.stack[0]
            .node_outputs
            .insert("report".into(), [("text".into(), json!(prose))].into());
        fs::write(
            dir.join("state.snapshot.json"),
            serde_json::to_vec(&state).unwrap(),
        )
        .unwrap();
        let path = dir.join("events.jsonl");
        let mut log = trollm_core::EventLog::open(&path, id).unwrap();
        log.append(
            EventType::Transition,
            json!({"key":"onVerified", "from":"report"}),
        )
        .unwrap();
        for _ in 0..400 {
            log.append(EventType::ModelCall, Value::Null).unwrap();
        }
        drop(log);
        let first = rt.observe_run(id.into(), 0).unwrap();
        assert!(first.older_events_omitted && first.has_more);
        assert_eq!(first.status.final_report.as_deref(), Some(prose.as_str()));
        assert_eq!(first.status.last_seq, 401);
        // A corrupt old prefix proves idle cached polls do not scan the journal.
        fs::write(&path, "malformed old prefix\n").unwrap();
        for _ in 0..10 {
            let idle = rt.observe_run(id.into(), 401).unwrap();
            assert!(idle.events.is_empty());
            assert_eq!(idle.status.final_report.as_deref(), Some(prose.as_str()));
        }
        assert!(rt.tail_events(id.into(), 401).is_err());
        assert!(rt.poll_status(id.into()).is_err());
        rt.invalidate_observation(id.into()).unwrap();
        assert!(rt.observe_run(id.into(), 401).is_err());
    }

    #[test]
    fn live_observation_follows_worker_activity_pause_success_and_failure() {
        let base = TrustTestDir::new("live-observation-lifecycle");
        let rt = crate::TrollmRuntime::new(base.0.to_string_lossy().into_owned(), None).unwrap();
        let id = "00000000-0000-0000-0000-000000000079";
        let run_dir = base.0.join("runs").join(id);
        fs::create_dir_all(&run_dir).unwrap();
        fs::write(run_dir.join("events.jsonl"), b"").unwrap();
        let (handle, _) = handle();
        rt.runs.insert(id.into(), handle.clone());
        let status = rt.observe_run(id.into(), 0).unwrap().status;
        assert!(status.worker_active);
        assert_eq!(status.activity, Some(RunActivityApi::Starting));
        assert!(rt.invalidate_observation(id.into()).is_err());
        *handle.status.lock().unwrap() = RunPhase::Paused {
            reason: "synthetic hold".into(),
        };
        let status = rt.observe_run(id.into(), 0).unwrap().status;
        assert!(status.worker_active && status.activity.is_none());
        handle.control.observation.lock().unwrap().final_report = Some("full report".into());
        *handle.status.lock().unwrap() = RunPhase::Succeeded;
        let status = rt.observe_run(id.into(), 0).unwrap().status;
        assert!(
            status.worker_active && status.activity.is_none(),
            "terminal phase is not an inactive-worker acknowledgement"
        );
        assert!(rt.poll_status(id.into()).unwrap().worker_active);
        assert_eq!(status.final_report.as_deref(), Some("full report"));
        *handle.status.lock().unwrap() = RunPhase::Failed {
            reason: "worker failed".into(),
        };
        let status = rt.observe_run(id.into(), 0).unwrap().status;
        assert!(matches!(status.phase, crate::RunPhaseApi::Failed { .. }));
        assert!(status.final_report.is_none());
        // Projection poisoning is an observation error, never a new dispatch.
        let projection = handle.control.observation.clone();
        let _ = std::thread::spawn(move || {
            let _view = projection.lock().unwrap();
            panic!("synthetic observation loss");
        })
        .join();
        assert!(rt.observe_run(id.into(), 0).is_err());
        assert!(matches!(
            *handle.status.lock().unwrap(),
            RunPhase::Failed { .. }
        ));
    }

    #[test]
    fn streamed_report_keeps_last_matching_node_and_rejects_later_corruption() {
        let base = TrustTestDir::new("bounded-report");
        let dir = base.0.join("runs/report");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("events.jsonl");
        let mut state = trollm_core::RunState::fresh("report", "flow@sha256:absent", "work");
        state.stack[0]
            .node_outputs
            .insert("first".into(), [("text".into(), json!("earlier"))].into());
        state.stack[0].node_outputs.insert(
            "last".into(),
            [
                ("z".into(), json!("zebra")),
                ("a".into(), json!("apple")),
                ("number".into(), json!(3)),
            ]
            .into(),
        );
        fs::write(
            dir.join("state.snapshot.json"),
            serde_json::to_vec(&state).unwrap(),
        )
        .unwrap();
        let mut journal = String::new();
        for (seq, node, key) in [
            (1, "first", "onVerified"),
            (2, "last", "onVerified"),
            (3, "first", "onError"),
        ] {
            journal.push_str(&format!(
                "{}\n",
                json!({"ts": 1, "runId": "report", "seq": seq,
                "type": "transition", "key": key, "from": node})
            ));
        }
        fs::write(&path, &journal).unwrap();
        assert_eq!(
            final_report(&base.0, "report").unwrap().as_deref(),
            Some("apple\nzebra")
        );
        // Selection is best effort when the snapshot is absent, as before.
        fs::remove_file(dir.join("state.snapshot.json")).unwrap();
        assert_eq!(final_report(&base.0, "report").unwrap(), None);
        journal.push_str("malformed\n");
        fs::write(&path, &journal).unwrap();
        assert!(final_report(&base.0, "report").is_err());
        assert_eq!(fs::read(&path).unwrap(), journal.as_bytes());
    }

    #[test]
    fn public_observers_reject_oversized_or_corrupt_journals_without_rewriting() {
        use crate::TrollmRuntime;
        let base = TrustTestDir::new("bounded-public-observers");
        let rt = TrollmRuntime::new(base.0.to_string_lossy().into_owned(), None).unwrap();
        let id = "00000000-0000-0000-0000-000000000058";
        let dir = base.0.join("trollm/runs").join(id);
        fs::create_dir_all(&dir).unwrap();
        let mut state = trollm_core::RunState::fresh(id, "flow@sha256:absent", "work");
        state.status = trollm_core::state::StateStatus::Succeeded;
        fs::write(
            dir.join("state.snapshot.json"),
            serde_json::to_vec(&state).unwrap(),
        )
        .unwrap();
        let path = dir.join("events.jsonl");
        let oversized = format!(
            "{}\n",
            json!({"ts": 1, "runId": id, "seq": 1,
            "type": "run_started", "padding": "x".repeat(trollm_core::events::MAX_EVENT_RECORD_BYTES)})
        );
        for bytes in [oversized.into_bytes(), b"malformed\n".to_vec()] {
            fs::write(&path, &bytes).unwrap();
            assert!(rt.tail_events(id.into(), u64::MAX).is_err());
            assert!(rt.poll_status(id.into()).is_err());
            // History remains a snapshot projection, not a journal reader.
            assert!(rt.run_summary(id.into()).unwrap().phase.is_some());
            assert_eq!(fs::read(&path).unwrap(), bytes);
        }
        // Sparse file tests the physical log-size precheck without a large buffer.
        let length = trollm_core::events::MAX_EVENT_LOG_BYTES + 1;
        fs::File::create(&path).unwrap().set_len(length).unwrap();
        assert!(rt
            .tail_events(id.into(), 0)
            .unwrap_err()
            .to_string()
            .contains("event log exceeds byte limit"));
        assert!(rt.poll_status(id.into()).is_err());
        assert_eq!(fs::metadata(&path).unwrap().len(), length);
    }

    fn audit_event() -> ProxyEvent {
        ProxyEvent::LiveCall {
            server: "test".into(),
            tool: "read".into(),
            args_hash: "hash".into(),
        }
    }

    #[test]
    fn proxy_audit_reopens_sequence_and_delimits_valid_tail() {
        let dir = TrustTestDir::new("audit-sequence");
        let path = dir.0.join("proxy.events.jsonl");
        let mut sink = proxy_sink(path.clone(), "run".into()).unwrap();
        sink(audit_event()).unwrap();
        drop(sink);
        let mut bytes = fs::read(&path).unwrap();
        bytes.pop(); // A complete JSON record without its final delimiter is valid.
        fs::write(&path, bytes).unwrap();
        let mut sink = proxy_sink(path.clone(), "run".into()).unwrap();
        sink(audit_event()).unwrap();
        drop(sink);
        let records: Vec<Value> = fs::read_to_string(path)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0]["seq"], 1);
        assert_eq!(records[1]["seq"], 2);
    }

    #[test]
    fn proxy_audit_rejects_directory_and_retains_corruption() {
        let dir = TrustTestDir::new("audit-corruption");
        assert!(proxy_sink(dir.0.clone(), "run".into()).is_err());
        let path = dir.0.join("proxy.events.jsonl");
        let record = json!({"ts":1,"runId":"run","seq":1,"type":"live_call", "server":"test", "tool":"read", "args_hash":"hash"}).to_string();
        for corrupt in [
            format!("{record}\n{{"),
            format!("{record}\n{record}\n"),
            record.replace("\"run\"", "\"other-run\""),
            record.replace("\"seq\":1", "\"seq\":18446744073709551615"),
            record.replace("live_call", "unknown_kind"),
        ] {
            fs::write(&path, &corrupt).unwrap();
            assert!(proxy_sink(path.clone(), "run".into()).is_err());
            assert_eq!(fs::read_to_string(&path).unwrap(), corrupt);
        }
        fs::write(&path, record.replace("\"seq\":1", "\"seq\":9")).unwrap();
        let mut sink = proxy_sink(path.clone(), "run".into()).unwrap();
        sink(audit_event()).unwrap();
        drop(sink);
        let last: Value =
            serde_json::from_str(fs::read_to_string(path).unwrap().lines().last().unwrap())
                .unwrap();
        assert_eq!(last["seq"], 10);
    }

    #[test]
    fn proxy_audit_record_limit_matches_scan_and_append() {
        let dir = TrustTestDir::new("audit-limit");
        let path = dir.0.join("proxy.events.jsonl");
        let mut sink = proxy_sink(path.clone(), "run".into()).unwrap();
        let oversized = ProxyEvent::LiveCall {
            server: "test".into(),
            tool: "x".repeat(MAX_PROXY_RECORD_BYTES),
            args_hash: "hash".into(),
        };
        assert!(sink(oversized)
            .unwrap_err()
            .to_string()
            .contains("record exceeds byte limit"));
        drop(sink);
        assert!(fs::read(&path).unwrap().is_empty());
        let oversized = vec![b'x'; MAX_PROXY_RECORD_BYTES + 1];
        fs::write(&path, &oversized).unwrap();
        assert!(proxy_sink(path.clone(), "run".into()).is_err());
        assert_eq!(fs::read(path).unwrap(), oversized);
    }

    #[test]
    fn proxy_audit_write_and_flush_failures_are_sticky() {
        struct BrokenWriter {
            flush_failure: bool,
            attempts: Arc<AtomicU64>,
        }
        impl std::io::Write for BrokenWriter {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.attempts.fetch_add(1, Ordering::SeqCst);
                if self.flush_failure {
                    Ok(bytes.len())
                } else {
                    Err(std::io::Error::other("injected write failure"))
                }
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Err(std::io::Error::other("injected flush failure"))
            }
        }
        for flush_failure in [false, true] {
            let attempts = Arc::new(AtomicU64::new(0));
            let mut sink = proxy_writer_sink(
                BrokenWriter {
                    flush_failure,
                    attempts: attempts.clone(),
                },
                "run".into(),
                0,
            );
            let error = format!("{:#}", sink(audit_event()).unwrap_err());
            assert!(error.contains(if flush_failure {
                "flush required"
            } else {
                "write required"
            }));
            assert!(sink(audit_event())
                .unwrap_err()
                .to_string()
                .contains("previously failed"));
            assert_eq!(attempts.load(Ordering::SeqCst), 1);
        }
    }

    #[test]
    fn optional_mcp_roots_load_and_revert_when_removed() {
        let base = TrustTestDir::new("reload");
        let path = base.0.join("trust/mcp-roots.pem");
        assert!(load_mcp_roots(&base.0).is_ok());
        fs::write(&path, tls_fixture::ROOT_1_PEM).unwrap();
        assert!(load_mcp_roots(&base.0).is_ok());
        fs::remove_file(path).unwrap();
        assert!(load_mcp_roots(&base.0).is_ok());
    }

    #[test]
    fn configured_mcp_roots_reject_invalid_oversized_and_non_regular_files() {
        let base = TrustTestDir::new("invalid");
        let path = base.0.join("trust/mcp-roots.pem");
        for invalid in [b"".as_slice(), b"not a certificate".as_slice()] {
            fs::write(&path, invalid).unwrap();
            let error = load_mcp_roots(&base.0)
                .err()
                .expect("invalid trust accepted");
            assert!(error.to_string().contains("invalid app MCP root"));
        }
        fs::write(&path, vec![b' '; 128 * 1024 + 1]).unwrap();
        let error = load_mcp_roots(&base.0)
            .err()
            .expect("oversized trust accepted");
        assert!(error.to_string().contains("exceed 128 KiB"));
        fs::remove_file(&path).unwrap();
        fs::create_dir(&path).unwrap();
        let error = load_mcp_roots(&base.0)
            .err()
            .expect("directory accepted as PEM");
        assert!(error.to_string().contains("regular PEM file"));
    }

    #[test]
    fn native_completion_fixture_uses_actual_terminal_writer() {
        let base = TrustTestDir::new("native-completion-writer");
        let mut yaml: serde_yaml::Value = serde_yaml::from_str(include_str!(
            "../../../flows/examples/minimal-summary.flow.yaml"
        ))
        .unwrap();
        yaml["id"] = serde_yaml::Value::String("native-completion-control".into());
        yaml["models"] = serde_yaml::from_str("{}").unwrap();
        yaml["entryStep"] = serde_yaml::Value::String("done".into());
        yaml["nodes"]
            .as_mapping_mut()
            .unwrap()
            .remove(serde_yaml::Value::String("summarize".into()));
        let plan = ExecutionPlan::from_yaml_str(&serde_yaml::to_string(&yaml).unwrap()).unwrap();
        let registry = TaskRegistry::from_yaml_str(
            "schemaVersion: ledger.task-registry.v1\ntasks: {}",
            &base.0,
        )
        .unwrap();
        let run_dir = RunDir::under(&base.0);
        let gateway = McpGateway::new(
            vec![],
            Default::default(),
            base.0.join("cache"),
            run_dir.artifacts_dir(),
            None,
        )
        .unwrap();
        let mut executor = Executor::new(
            plan,
            registry,
            "{}".into(),
            SlotMap::<trollm_models::MockModel>::new(),
            gateway,
            run_dir.clone(),
        )
        .unwrap();
        assert!(matches!(
            executor.run_until_blocked().unwrap(),
            RunStatus::Succeeded
        ));
        let snapshot = fs::read_to_string(run_dir.snapshot_path()).unwrap();
        let journal = fs::read_to_string(run_dir.path().join("events.jsonl")).unwrap();
        let state: Value = serde_json::from_str(&snapshot).unwrap();
        let last: Value = serde_json::from_str(journal.lines().last().unwrap()).unwrap();
        assert_eq!(state["runId"], last["runId"]);
        assert_eq!(state["eventWatermark"], last["seq"]);
        assert_eq!(last["type"], "run_ended");
        let fixture = json!({"provenance":"actual core Executor terminal snapshot and EventLog serialization; DECLARED source control only", "snapshotJson":snapshot, "journalJsonl":journal});
        let retained: Value = serde_json::from_str(include_str!(
            "../../../scripts/android/fixtures/native-completion.json"
        ))
        .unwrap();
        let retained_raw: Value =
            serde_json::from_str(retained["snapshotJson"].as_str().unwrap()).unwrap();
        let retained_state: trollm_core::RunState =
            serde_json::from_value(retained_raw.clone()).unwrap();
        assert_eq!(
            serde_json::to_value(retained_state).unwrap(),
            retained_raw,
            "retained Node fixture must have the complete actual serializer shape"
        );
        let retained_last: Value = serde_json::from_str(
            retained["journalJsonl"]
                .as_str()
                .unwrap()
                .lines()
                .last()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(retained_raw["eventWatermark"], retained_last["seq"]);
        assert_eq!(retained_raw["runId"], retained_last["runId"]);
        if let Some(path) = std::env::var_os("TROLLM_WRITE_COMPLETION_FIXTURE") {
            fs::write(path, serde_json::to_vec_pretty(&fixture).unwrap()).unwrap();
        }
    }

    #[test]
    fn controlled_gateway_forwards_task_result_artifact_attempt_and_dispatch() {
        use trollm_mcp::ToolGateway;
        let base = TrustTestDir::new("task-result-forwarding");
        let artifacts_dir = base.0.join("artifacts");
        let server = super::gateway_fixture::MockMcpServer::start_with_tools_response(json!({
            "result": {"tools": [{"name":"read","inputSchema":{"type":"object"}}]}
        }));
        let inner = McpGateway::new(
            vec![trollm_mcp::ServerConfig::for_local_test(
                "fixture",
                &server.url,
            )],
            trollm_mcp::PolicySet::from_yaml_str(
                "fixture.read: {class: read, resultArtifact: browser-text}",
            )
            .unwrap(),
            base.0.join("cache"),
            artifacts_dir.clone(),
            None,
        )
        .unwrap();
        let (handle, _) = handle();
        let mut gateway = ControlledGateway {
            inner,
            ctl: handle.control.clone(),
        };
        gateway
            .begin_result_artifact_attempt("fixture", "read", "selected-original")
            .unwrap();
        let meta = artifacts_dir.join("selected-original.json.meta.json");
        assert_eq!(
            serde_json::from_slice::<Value>(&fs::read(&meta).unwrap()).unwrap()["complete"],
            false
        );
        fs::remove_file(&meta).unwrap();
        // A real loopback catalog admits the tool before result storage starts.
        // Complete task-owned storage proves the wrapper forwarded the override
        // through actual discovery and dispatch, not only attempt creation.
        let result = gateway
            .call_with_result_artifact(
                &trollm_mcp::ToolCall {
                    server: "fixture".into(),
                    tool: "read".into(),
                    args: json!({}),
                },
                Some("selected-original"),
            )
            .unwrap();
        assert_eq!(server.count("call:read"), 1);
        assert_eq!(
            serde_json::from_slice::<Value>(&fs::read(&meta).unwrap()).unwrap()["complete"],
            true
        );
        assert!(result
            .artifacts
            .contains(&artifacts_dir.join("selected-original.json")));
        assert!(!artifacts_dir.join("browser-text.json").exists());
    }

    #[test]
    fn tool_free_gateway_ignores_optional_mcp_trust_configuration() {
        let base = TrustTestDir::new("tool-free");
        fs::write(base.0.join("trust/mcp-roots.pem"), b"invalid").unwrap();
        let source = include_str!("../../../flows/examples/minimal-summary.flow.yaml");
        let run_dir = RunDir::under(&base.0);
        fs::create_dir_all(run_dir.path()).unwrap();
        let run_plan = |plan_source: &str| RunPlan {
            configuration: None,
            argument_constraints: Default::default(),
            selected_preparer: None,
            plan: ExecutionPlan::from_yaml_str(plan_source).unwrap(),
            registry: TaskRegistry::from_yaml_str(
                include_str!("../../../flows/tasks/minimal-summary.tasks.yaml"),
                &base.0,
            )
            .unwrap(),
        };
        assert!(build_gateway(&run_plan(source), &base.0, &run_dir, None, false).is_ok());
        let with_mcp = source.replace(
            "servers: []",
            "servers: [{name: test, url: 'https://example.test/mcp'}]",
        );
        let error = build_gateway(&run_plan(&with_mcp), &base.0, &run_dir, None, false)
            .err()
            .expect("MCP gateway ignored broken configured roots");
        assert!(error.to_string().contains("invalid app MCP root"));
    }

    #[cfg(all(feature = "stub", not(feature = "llama")))]
    #[test]
    fn drive_clears_starting_during_pre_generation_backoff_and_user_resume() {
        let base = TrustTestDir::new("backoff-activity");
        fs::create_dir_all(base.0.join("prompts")).unwrap();
        fs::write(
            base.0.join("prompts/minimal-summary.md"),
            "{{ backoff_probe_unbound }}",
        )
        .unwrap();
        let source = include_str!("../../../flows/examples/minimal-summary.flow.yaml")
            .replace("timeoutS: 60", "timeoutS: 600")
            .replace(
                "    onVerified: done",
                "    onVerified: done\n    onError: { retry: 1, backoffS: 300, then: done }",
            );
        let run_plan = RunPlan {
            configuration: None,
            argument_constraints: Default::default(),
            selected_preparer: None,
            plan: ExecutionPlan::from_yaml_str(&source).unwrap(),
            registry: TaskRegistry::from_yaml_str(
                include_str!("../../../flows/tasks/minimal-summary.tasks.yaml"),
                &base.0,
            )
            .unwrap(),
        };
        let run_dir = RunDir::under(&base.0);
        run_dir.create_dirs().unwrap();
        let events_path = run_dir.path().join("events.jsonl");
        let (handle, commands) = handle();
        let ctl = handle.control.clone();
        let worker_base = base.0.clone();
        let (done, result) = channel();
        let worker = std::thread::spawn(move || {
            let guard = WorkerGuard(ctl.clone());
            let outcome = drive(
                run_plan,
                worker_base,
                run_dir,
                String::new(),
                None,
                (ctl, commands),
                false,
            );
            drop(guard);
            let _ = done.send(outcome);
        });
        // Always wake/cancel the 300-second wait, including if an assertion fails.
        let observed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let wait_for = |ready: &dyn Fn() -> bool| {
                let deadline = Instant::now() + Duration::from_secs(2);
                while !ready() {
                    assert!(
                        Instant::now() < deadline,
                        "worker did not reach expected state"
                    );
                    std::thread::sleep(Duration::from_millis(1));
                }
            };
            wait_for(&|| {
                fs::read_to_string(&events_path)
                    .is_ok_and(|events| events.contains("prompt render: template variable"))
            });
            assert!(handle.control.worker_active());
            assert_eq!(*handle.status.lock().unwrap(), RunPhase::Running);
            assert_eq!(handle.control.activity(), None);
            handle.pause();
            wait_for(&|| matches!(*handle.status.lock().unwrap(), RunPhase::Paused { .. }));
            assert_eq!(handle.control.activity(), None);
            handle.resume();
            wait_for(&|| matches!(*handle.status.lock().unwrap(), RunPhase::Running));
            assert!(handle.control.worker_active());
            assert_eq!(handle.control.activity(), None);
        }));
        handle.cancel();
        let outcome = result.recv_timeout(Duration::from_secs(2));
        // Join only after completion was reported, keeping failure cleanup bounded.
        if outcome.is_ok() {
            worker.join().unwrap();
        }
        if let Err(panic) = observed {
            std::panic::resume_unwind(panic);
        }
        outcome
            .expect("cancel did not wake the backoff worker")
            .unwrap();
        assert!(!handle.control.worker_active());
        assert_eq!(
            *handle.status.lock().unwrap(),
            RunPhase::Failed {
                reason: "cancelled".into()
            }
        );
        let events: Vec<Value> = fs::read_to_string(events_path)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(
            events
                .iter()
                .filter(|event| event["type"] == "error")
                .count(),
            1
        );
        assert!(events
            .iter()
            .any(|event| event["class"] == "verification-failed"
                && event["detail"]
                    .as_str()
                    .is_some_and(|detail| detail.contains("backoff_probe_unbound"))));
        assert!(!events.iter().any(|event| matches!(
            event["type"].as_str(),
            Some("model_call" | "tool_call_intent" | "tool_call_result")
        )));
    }

    #[cfg(all(feature = "stub", not(feature = "llama")))]
    #[test]
    fn cancelled_snapshot_failure_is_visible_live_and_after_reopen() {
        let base = TrustTestDir::new("cancel-snapshot-failure");
        fs::create_dir_all(base.0.join("prompts")).unwrap();
        fs::write(
            base.0.join("prompts/minimal-summary.md"),
            include_str!("../../../flows/tasks/prompts/minimal-summary.md"),
        )
        .unwrap();
        let run_dir = RunDir::under(&base.0);
        run_dir.create_dirs().unwrap();
        let run_id = run_dir.run_id().to_string();
        let run_path = run_dir.path().to_path_buf();
        let run_plan = RunPlan {
            configuration: None,
            argument_constraints: Default::default(),
            selected_preparer: None,
            plan: ExecutionPlan::from_yaml_str(include_str!(
                "../../../flows/examples/minimal-summary.flow.yaml"
            ))
            .unwrap(),
            registry: TaskRegistry::from_yaml_str(
                include_str!("../../../flows/tasks/minimal-summary.tasks.yaml"),
                &base.0,
            )
            .unwrap(),
        };
        let (handle, commands) = handle();
        handle.pause();
        let ctl = handle.control.clone();
        let worker_base = base.0.clone();
        let worker_path = run_path.clone();
        let (worker_done_tx, worker_done_rx) = channel();
        let worker = std::thread::spawn(move || {
            let result = drive(
                run_plan,
                worker_base,
                run_dir,
                String::new(),
                None,
                (ctl.clone(), commands),
                false,
            );
            let outcome = match result {
                Err(error) => {
                    let reason = format!("{error:#}");
                    publish_worker_failure(&worker_path, &ctl, error);
                    reason
                }
                Ok(()) => "drive unexpectedly returned success".into(),
            };
            worker_done_tx.send(outcome).unwrap();
        });
        let deadline = Instant::now() + Duration::from_secs(2);
        while !matches!(*handle.status.lock().unwrap(), RunPhase::Paused { .. }) {
            match worker_done_rx.try_recv() {
                Ok(reason) => panic!("worker exited before the hold: {reason}"),
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    panic!("worker exited before the hold without reporting an outcome")
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
            }
            assert!(Instant::now() < deadline, "worker did not reach the hold");
            std::thread::yield_now();
        }
        let snapshot: trollm_core::RunState =
            serde_json::from_slice(&fs::read(run_path.join("state.snapshot.json")).unwrap())
                .unwrap();
        assert_eq!(snapshot.status, trollm_core::state::StateStatus::Running);
        fs::create_dir(run_path.join("state.snapshot.tmp")).unwrap();
        handle.cancel();
        let worker_error = worker_done_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_ne!(worker_error, "drive unexpectedly returned success");
        worker.join().unwrap();

        let live = handle.status.lock().unwrap().clone();
        let cold = status_from_disk(&base.0, &run_id).unwrap().unwrap();
        assert_eq!(live, cold);
        assert!(matches!(live, RunPhase::Failed { reason } if reason.contains("run worker error")));
        assert!(run_path.join(WORKER_FAILURE_FILE).exists());
    }

    fn handle() -> (Arc<RunHandle>, Receiver<RunCommand>) {
        let status = Arc::new(Mutex::new(RunPhase::Running));
        let control = Arc::new(RunControl {
            hold_ns: AtomicU64::new(0),
            observation: Arc::new(Mutex::new(crate::observation::Projection::default())),
            cancel: AtomicBool::new(false),
            cancellation_cause: Mutex::new(None),
            worker_active: AtomicBool::new(true),
            selected_preparer: None,
            volatile_worker_failure: AtomicBool::new(false),
            activity: Mutex::new(Some(RunActivityApi::Starting)),
            preparation_activity: Mutex::new(HashMap::new()),
            pause_requested: AtomicBool::new(false),
            gate: Mutex::new(PauseGate::default()),
            gate_cond: Condvar::new(),
            status: status.clone(),
        });
        let (tx, rx) = channel();
        (
            Arc::new(RunHandle {
                status,
                control,
                cmd: Mutex::new(Some(tx)),
                run_dir: PathBuf::new(),
            }),
            rx,
        )
    }

    #[test]
    fn selected_case_expiry_wakes_cooperative_hold_and_waits_for_real_worker_guard() {
        let (handle, _) = handle();
        assert!(handle.control.selected_invocation_binding().is_err());
        let mut handle = Arc::try_unwrap(handle).ok().unwrap();
        let (preparer, clock) = crate::collection_provider::controls::actor_for_control();
        Arc::get_mut(&mut handle.control).unwrap().selected_preparer = Some(preparer);
        let control = handle.control;
        let original_binding = control.selected_invocation_binding().unwrap();
        assert_eq!(original_binding.0, "a".repeat(64));
        assert!(!original_binding.1.is_empty());
        control.request_pause();
        let worker_control = control.clone();
        let (tx, rx) = channel();
        let worker = std::thread::spawn(move || {
            let _worker = WorkerGuard(worker_control.clone());
            tx.send(worker_control.before_boundary().is_err()).unwrap();
        });
        let deadline = Instant::now() + Duration::from_secs(2);
        while !matches!(*control.status.lock().unwrap(), RunPhase::Paused { .. }) {
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
        assert!(control.worker_active());
        clock.now.store(u64::MAX, Ordering::SeqCst);
        let cancelled = rx.recv_timeout(Duration::from_secs(2));
        // On a regression, unblock and join rather than leak a test worker.
        control.request_cancel();
        worker.join().unwrap();
        assert!(cancelled.unwrap());
        assert_eq!(
            *control.cancellation_cause.lock().unwrap(),
            Some("SELECTED_ACTOR_EXPIRED")
        );
        assert!(!control.worker_active());
        assert_eq!(
            control.selected_invocation_binding().unwrap(),
            original_binding
        );
        clock.now.store(0, Ordering::SeqCst);
        assert!(
            control.is_cancelled(),
            "expired actor must not silently renew"
        );
    }

    #[test]
    fn cancellation_cause_keeps_original_classification_and_unknown_on_disk() {
        let (handle, _) = handle();
        handle.control.latch_cancel("SELECTED_CLOCK_STALE");
        handle.control.request_cancel();
        let directory = std::env::temp_dir().join(format!(
            "cancel-cause-{}",
            trollm_core::RunDir::under(Path::new("unused")).run_id()
        ));
        fs::create_dir(&directory).unwrap();
        retain_cancellation_cause(&directory, &handle.control).unwrap();
        let value: Value =
            serde_json::from_slice(&fs::read(directory.join("cancellation-cause.json")).unwrap())
                .unwrap();
        assert_eq!(value["cause"], "SELECTED_CLOCK_STALE");
        assert_eq!(value["effectDisposition"], "unknown");
        assert_eq!(value["meaning"], "cooperative-stop-cause-only");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn selected_model_wrapper_spans_single_long_and_three_item_waits_with_owned_refresh() {
        struct Environment(Arc<crate::collection_clock::CalibratedClock>);
        impl crate::collection_provider::InstalledSelectedEnvironment for Environment {
            fn start_clock_maintenance(&self) -> Result<()> {
                self.0.start_maintenance()
            }
            fn stop_clock_maintenance(&self) {
                self.0.stop_maintenance();
            }
            fn current_utc_upper_ms(&self) -> Result<u64> {
                Ok(self.0.sample()?.0)
            }
            fn check_current_authority(
                &self,
                _: &crate::collection::SelectionRecord,
                _: &trollm_mcp::preparation::PreparationContext,
            ) -> Result<()> {
                bail!("no effect authority in model control")
            }
        }
        struct WaitingModel;
        impl ModelSlot for WaitingModel {
            fn generate(&mut self, request: GenerateRequest) -> Result<GenerateResponse> {
                std::thread::sleep(if request.prompt == "single-long-call" {
                    Duration::from_millis(1300)
                } else {
                    Duration::from_millis(500)
                });
                Ok(GenerateResponse {
                    text: "declared-only".into(),
                    tokens_in: 1,
                    tokens_out: 1,
                    finish: trollm_models::FinishReason::Stop,
                    capacity: None,
                    diagnostics: None,
                })
            }
        }
        let (clock, server, calls, directory) =
            crate::collection_clock::controls::clock_fixture_responses("refresh", 0);
        let clock = Arc::new(clock);
        clock.acquire().unwrap();
        let (handle, _) = handle();
        let mut handle = Arc::try_unwrap(handle).ok().unwrap();
        Arc::get_mut(&mut handle.control).unwrap().selected_preparer =
            Some(crate::collection_provider::controls::actor_for_environment(
                Arc::new(Environment(clock)),
            ));
        handle
            .control
            .selected_preparer
            .as_ref()
            .unwrap()
            .start_clock_maintenance()
            .unwrap();
        let worker = WorkerGuard(handle.control.clone());
        let mut model = ControlledModel {
            slot: "coder".into(),
            inner: Box::new(WaitingModel),
            ctl: handle.control.clone(),
        };
        let mut single = request();
        single.prompt = "single-long-call".into();
        assert_eq!(model.generate(single).unwrap().text, "declared-only");
        for _ in 0..3 {
            assert_eq!(model.generate(request()).unwrap().text, "declared-only");
        }
        assert!(!handle.control.is_cancelled());
        assert!(handle.control.worker_active());
        drop(worker);
        assert!(!handle.control.worker_active());
        assert!(calls.load(Ordering::SeqCst) >= 3);
        server.join().unwrap();
        fs::remove_dir_all(directory).unwrap();
    }

    fn inactive_handle(phase: RunPhase) -> Arc<RunHandle> {
        let (handle, _) = handle();
        *handle.status.lock().unwrap() = phase;
        drop(WorkerGuard(handle.control.clone()));
        handle
    }

    fn churn(registry: &RunRegistry, count: usize) {
        for index in 0..count {
            registry.insert(
                format!("completed-{index}"),
                inactive_handle(RunPhase::Succeeded),
            );
        }
    }

    #[test]
    fn registry_bounds_inactive_runs_and_releases_evicted_handles() {
        let registry = RunRegistry::default();
        let first = inactive_handle(RunPhase::Succeeded);
        let weak = Arc::downgrade(&first);
        registry.insert("first".into(), first);
        churn(&registry, 2000);
        assert!(weak.upgrade().is_none());
        assert!(registry.get("first").is_none());
        assert!(registry.get("completed-1967").is_none());
        assert!(registry.get("completed-1968").is_some());
        let state = registry.state.lock().unwrap();
        assert_eq!(state.runs.len(), INACTIVE_RUN_CACHE_LIMIT);
        assert_eq!(state.registration_order.len(), state.runs.len());
    }

    #[test]
    fn registry_retains_live_running_paused_and_terminal_workers() {
        let registry = RunRegistry::default();
        let (running, _) = handle();
        let (paused, _) = handle();
        *paused.status.lock().unwrap() = RunPhase::Paused {
            reason: "hold".into(),
        };
        let (terminal, _) = handle();
        *terminal.status.lock().unwrap() = RunPhase::Succeeded;
        for (id, control) in [
            ("running", &running),
            ("paused", &paused),
            ("terminal", &terminal),
        ] {
            registry.insert(id.into(), control.clone());
        }
        churn(&registry, 1000);
        for id in ["running", "paused", "terminal"] {
            assert!(registry.get(id).is_some());
        }
        assert!(registry.has_active());
        // Worker teardown, rather than terminal publication, makes all three
        // eligible. Both reads and admission checks prune without another run.
        for control in [&running, &paused, &terminal] {
            drop(WorkerGuard(control.control.clone()));
        }
        assert!(!registry.has_active());
        for id in ["running", "paused", "terminal"] {
            assert!(registry.get(id).is_none());
        }
        assert_eq!(
            registry.state.lock().unwrap().runs.len(),
            INACTIVE_RUN_CACHE_LIMIT
        );
    }

    #[test]
    fn registry_get_prunes_workers_that_finished_after_insertion() {
        let registry = RunRegistry::default();
        let handles: Vec<_> = (0..100).map(|_| handle().0).collect();
        for (index, handle) in handles.iter().enumerate() {
            registry.insert(index.to_string(), handle.clone());
        }
        for handle in handles {
            drop(WorkerGuard(handle.control.clone()));
        }
        assert!(registry.get("0").is_none());
        assert!(registry.get("99").is_some());
        assert_eq!(
            registry.state.lock().unwrap().runs.len(),
            INACTIVE_RUN_CACHE_LIMIT
        );
    }

    #[test]
    fn registry_replacement_gets_new_age_without_duplicate_order_entries() {
        let registry = RunRegistry::default();
        churn(&registry, INACTIVE_RUN_CACHE_LIMIT);
        let replacement = inactive_handle(RunPhase::Failed {
            reason: "replacement".into(),
        });
        for _ in 0..100 {
            registry.insert("completed-0".into(), replacement.clone());
        }
        registry.insert("new".into(), inactive_handle(RunPhase::Succeeded));
        assert!(registry.get("completed-1").is_none());
        assert!(Arc::ptr_eq(
            &registry.get("completed-0").unwrap(),
            &replacement
        ));
        let state = registry.state.lock().unwrap();
        assert_eq!(state.runs.len(), INACTIVE_RUN_CACHE_LIMIT);
        assert_eq!(state.registration_order.len(), state.runs.len());
        assert_eq!(
            state
                .registration_order
                .iter()
                .filter(|id| *id == "completed-0")
                .count(),
            1
        );
    }

    #[test]
    fn evicted_terminal_runs_keep_public_status_and_history() {
        use crate::{RunPhaseApi, TrollmRuntime};
        let base = TrustTestDir::new("evicted-status");
        let rt = TrollmRuntime::new(base.0.to_string_lossy().into_owned(), None).unwrap();
        let phases = [
            RunPhase::Succeeded,
            RunPhase::Failed {
                reason: "executor failed".into(),
            },
            RunPhase::Failed {
                reason: "cancelled".into(),
            },
            RunPhase::Failed {
                reason: "run worker error: startup failed".into(),
            },
        ];
        let mut ids = Vec::new();
        for (index, phase) in phases.iter().enumerate() {
            let id = format!("00000000-0000-0000-0000-{index:012}");
            let dir = base.0.join("trollm/runs").join(&id);
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("events.jsonl"), b"").unwrap();
            let mut state = trollm_core::RunState::fresh(&id, "flow@sha256:absent", "work");
            match phase {
                RunPhase::Succeeded => state.status = trollm_core::state::StateStatus::Succeeded,
                RunPhase::Failed { reason } => {
                    state.status = trollm_core::state::StateStatus::Failed;
                    state.terminal_reason = Some(reason.clone());
                }
                _ => unreachable!(),
            }
            fs::write(
                dir.join("state.snapshot.json"),
                serde_json::to_vec(&state).unwrap(),
            )
            .unwrap();
            let handle = inactive_handle(phase.clone());
            if index == 3 {
                // Worker failure must override an older nonterminal snapshot.
                state.status = trollm_core::state::StateStatus::Running;
                fs::write(
                    dir.join("state.snapshot.json"),
                    serde_json::to_vec(&state).unwrap(),
                )
                .unwrap();
                publish_worker_failure(&dir, &handle.control, anyhow!("startup failed"));
            }
            rt.runs.insert(id.clone(), handle);
            ids.push(id);
        }
        let check = || {
            // Missing bundles make any accidental terminal reconstruction fail.
            for id in ids.iter().take(3) {
                rt.resume(id.clone()).unwrap();
            }
            for (id, expected) in ids.iter().zip(&phases) {
                let status = rt.poll_status(id.clone()).unwrap();
                let summary = rt.run_summary(id.clone()).unwrap();
                assert!(!status.worker_active);
                assert!(!summary.worker_active);
                for phase in [status.phase, summary.phase.unwrap()] {
                    match (phase, expected) {
                        (RunPhaseApi::Succeeded, RunPhase::Succeeded) => {}
                        (RunPhaseApi::Failed { reason }, RunPhase::Failed { reason: expected }) => {
                            assert_eq!(&reason, expected)
                        }
                        other => panic!("unexpected terminal projection: {other:?}"),
                    }
                }
            }
        };
        check();
        churn(&rt.runs, 1000);
        for id in &ids {
            assert!(rt.runs.get(id).is_none());
        }
        check();
    }

    #[test]
    fn registry_keeps_unpersisted_worker_failure_outside_cache_bound() {
        use crate::{RunPhaseApi, TrollmRuntime};
        let base = TrustTestDir::new("volatile-failure");
        let rt = TrollmRuntime::new(base.0.to_string_lossy().into_owned(), None).unwrap();
        let id = "00000000-0000-0000-0000-000000000001";
        let dir = base.0.join("trollm/runs").join(id);
        fs::create_dir_all(dir.join(WORKER_FAILURE_FILE)).unwrap();
        let (failed, _) = handle();
        publish_worker_failure(&dir, &failed.control, anyhow!("unique volatile diagnostic"));
        drop(WorkerGuard(failed.control.clone()));
        rt.runs.insert(id.into(), failed.clone());
        churn(&rt.runs, 1000);
        assert_eq!(
            rt.runs.state.lock().unwrap().runs.len(),
            INACTIVE_RUN_CACHE_LIMIT + 1
        );
        assert!(Arc::ptr_eq(&rt.runs.get(id).unwrap(), &failed));
        assert!(!rt.has_active_run());
        // The pinned diagnostic must bypass even a broken snapshot read.
        fs::write(dir.join("state.snapshot.json"), b"invalid snapshot").unwrap();
        rt.resume(id.into()).unwrap();
        fs::remove_file(dir.join("state.snapshot.json")).unwrap();
        rt.resume(id.into()).unwrap();
        let status = rt.poll_status(id.into()).unwrap();
        let summary = rt.run_summary(id.into()).unwrap();
        for phase in [status.phase, summary.phase.unwrap()] {
            assert!(
                matches!(phase, RunPhaseApi::Failed { reason } if reason.contains("unique volatile diagnostic") && reason.contains("could not persist failure"))
            );
        }
    }

    #[test]
    fn worker_failure_resume_reconstructs_unfinished_saved_progress_once() {
        use crate::TrollmRuntime;
        use trollm_core::state::StateStatus;
        let base = TrustTestDir::new("worker-failure-resume");
        for (index, saved_status) in [StateStatus::Running, StateStatus::Paused]
            .into_iter()
            .enumerate()
        {
            for evicted in [false, true] {
                let rt = TrollmRuntime::new(base.0.to_string_lossy().into_owned(), None).unwrap();
                let id = format!("recover-{index}-{evicted}");
                let dir = base.0.join("trollm/runs").join(&id);
                fs::create_dir_all(&dir).unwrap();
                let mut state = trollm_core::RunState::fresh(&id, "flow@sha256:absent", "work");
                state.status = saved_status.clone();
                fs::write(
                    dir.join("state.snapshot.json"),
                    serde_json::to_vec(&state).unwrap(),
                )
                .unwrap();
                let failed = inactive_handle(RunPhase::Running);
                publish_worker_failure(
                    &dir,
                    &failed.control,
                    anyhow!("recoverable startup failure"),
                );
                rt.runs.insert(id.clone(), failed);
                if evicted {
                    churn(&rt.runs, 100);
                    assert!(rt.runs.get(&id).is_none());
                }
                let constructions = AtomicU64::new(0);
                for _ in 0..2 {
                    rt.resume_or_reconstruct(id.clone(), || {
                        constructions.fetch_add(1, Ordering::SeqCst);
                        Ok(handle().0)
                    })
                    .unwrap();
                }
                assert_eq!(constructions.load(Ordering::SeqCst), 1);
                assert!(rt.runs.get(&id).unwrap().control.worker_active());
            }
        }
    }

    #[test]
    fn terminal_snapshot_resume_preserves_worker_failure_sidecar() {
        use crate::TrollmRuntime;
        let base = TrustTestDir::new("terminal-worker-failure-resume");
        let rt = TrollmRuntime::new(base.0.to_string_lossy().into_owned(), None).unwrap();
        let id = "00000000-0000-0000-0000-000000000001";
        let dir = base.0.join("trollm/runs").join(id);
        fs::create_dir_all(&dir).unwrap();
        let mut state = trollm_core::RunState::fresh(id, "flow@sha256:absent", "work");
        state.status = trollm_core::state::StateStatus::Failed;
        state.terminal_reason = Some("cancelled".into());
        fs::write(
            dir.join("state.snapshot.json"),
            serde_json::to_vec(&state).unwrap(),
        )
        .unwrap();
        let failed = inactive_handle(RunPhase::Running);
        publish_worker_failure(&dir, &failed.control, anyhow!("teardown failure"));
        let sidecar = fs::read(dir.join(WORKER_FAILURE_FILE)).unwrap();
        rt.runs.insert(id.into(), failed);
        rt.resume(id.into()).unwrap();
        churn(&rt.runs, 100);
        assert!(rt.runs.get(id).is_none());
        rt.resume(id.into()).unwrap();
        assert_eq!(fs::read(dir.join(WORKER_FAILURE_FILE)).unwrap(), sidecar);
    }

    #[test]
    fn worker_failure_without_snapshot_resume_stays_not_found() {
        use crate::{TrollmError, TrollmRuntime};
        let base = TrustTestDir::new("worker-failure-no-snapshot-resume");
        let rt = TrollmRuntime::new(base.0.to_string_lossy().into_owned(), None).unwrap();
        let id = "00000000-0000-0000-0000-000000000001";
        let dir = base.0.join("trollm/runs").join(id);
        fs::create_dir_all(&dir).unwrap();
        let failed = inactive_handle(RunPhase::Running);
        publish_worker_failure(
            &dir,
            &failed.control,
            anyhow!("failed before first snapshot"),
        );
        rt.runs.insert(id.into(), failed);
        assert!(matches!(
            rt.resume(id.into()),
            Err(TrollmError::NotFound(_))
        ));
        churn(&rt.runs, 100);
        assert!(rt.runs.get(id).is_none());
        assert!(matches!(
            rt.resume(id.into()),
            Err(TrollmError::NotFound(_))
        ));
    }

    #[test]
    fn live_controls_still_work_after_registry_churn() {
        const RUNNING_ID: &str = "00000000-0000-0000-0000-000000000001";
        const PAUSED_ID: &str = "00000000-0000-0000-0000-000000000002";
        use crate::TrollmRuntime;
        let base = TrustTestDir::new("live-controls-after-churn");
        let rt = TrollmRuntime::new(base.0.to_string_lossy().into_owned(), None).unwrap();
        let (running, running_commands) = handle();
        let (paused, paused_commands) = handle();
        paused.control.gate.lock().unwrap().plan_paused = true;
        *paused.status.lock().unwrap() = RunPhase::Paused {
            reason: "authored pause".into(),
        };
        rt.runs.insert(RUNNING_ID.into(), running.clone());
        rt.runs.insert(PAUSED_ID.into(), paused.clone());
        churn(&rt.runs, 100);
        rt.pause(RUNNING_ID.into()).unwrap();
        assert!(running.control.pause_requested.load(Ordering::SeqCst));
        rt.resume(RUNNING_ID.into()).unwrap();
        assert!(!running.control.pause_requested.load(Ordering::SeqCst));
        assert!(running_commands.try_recv().is_err());
        rt.pause(PAUSED_ID.into()).unwrap();
        rt.resume(PAUSED_ID.into()).unwrap();
        assert!(!paused.control.pause_requested.load(Ordering::SeqCst));
        assert!(matches!(*paused.status.lock().unwrap(), RunPhase::Running));
        assert!(matches!(paused_commands.try_recv(), Ok(RunCommand::Resume)));
        for (id, handle, commands) in [
            (RUNNING_ID, running, running_commands),
            (PAUSED_ID, paused, paused_commands),
        ] {
            rt.cancel(id.into()).unwrap();
            assert!(handle.control.is_cancelled());
            assert!(matches!(commands.try_recv(), Ok(RunCommand::Cancel)));
        }
    }

    #[test]
    fn ordinary_model_digests_are_not_hashed_twice() {
        let base = std::env::temp_dir().join(format!("trollm-model-digest-{}", std::process::id()));
        fs::create_dir_all(base.join("models")).unwrap();
        let weights = base.join("models/coder.gguf");
        let manifest = base.join("models/coder.gguf.sha256");
        // Independent, standard SHA-256 test vector for ASCII "abc".
        let digest = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        fs::write(&weights, b"abc").unwrap();
        for value in [
            digest.to_string(),
            format!("{}  coder.gguf\n", digest.to_uppercase()),
        ] {
            fs::write(&manifest, value).unwrap();
            assert_eq!(
                verify_model_weights(
                    &base,
                    "coder",
                    &handle().0.control,
                    &GenerationBudget::new(None)
                )
                .unwrap(),
                weights
            );
        }
        fs::write(&weights, b"abd").unwrap();
        assert!(verify_model_weights(
            &base,
            "coder",
            &handle().0.control,
            &GenerationBudget::new(None)
        )
        .unwrap_err()
        .to_string()
        .contains("mismatch"));
        fs::remove_dir_all(base).unwrap();
    }

    fn request() -> GenerateRequest {
        GenerateRequest {
            prompt: "hello".into(),
            grammar_gbnf: None,
            max_tokens: 8,
            max_duration: None,
            stop: vec![],
        }
    }

    const ABC_DIGEST: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

    struct RecordingModel {
        ctl: Arc<RunControl>,
        dropped: Arc<AtomicU64>,
    }

    impl ModelSlot for RecordingModel {
        fn generate(&mut self, _: GenerateRequest) -> Result<GenerateResponse> {
            assert_eq!(
                self.ctl.activity(),
                Some(RunActivityApi::Generating {
                    slot: "coder".into()
                })
            );
            Ok(GenerateResponse {
                text: "fake response".into(),
                tokens_in: 1,
                tokens_out: 1,
                finish: trollm_models::FinishReason::Stop,
                capacity: None,
                diagnostics: None,
            })
        }
    }

    impl Drop for RecordingModel {
        fn drop(&mut self) {
            self.dropped.fetch_add(1, Ordering::SeqCst);
        }
    }

    fn lazy_fixture(
        path: PathBuf,
        ctl: Arc<RunControl>,
        loads: Arc<AtomicU64>,
        dropped: Arc<AtomicU64>,
    ) -> LazyModel {
        let loader_control = ctl.clone();
        LazyModel {
            base: PathBuf::new(),
            slot: "coder".into(),
            weights: Some((path, ABC_DIGEST.into())),
            loader: Box::new(move |_| {
                assert_eq!(
                    loader_control.activity(),
                    Some(RunActivityApi::LoadingModel {
                        slot: "coder".into()
                    })
                );
                loads.fetch_add(1, Ordering::SeqCst);
                Ok(Box::new(RecordingModel {
                    ctl: loader_control.clone(),
                    dropped: dropped.clone(),
                }))
            }),
            loaded: None,
            ctl,
        }
    }

    #[test]
    fn preparation_selects_only_referenced_slots_coder_first_with_two_slot_bound() {
        let mut plan: ExecutionPlan = serde_yaml::from_str(include_str!(
            "../../../flows/examples/minimal-summary.flow.yaml"
        ))
        .unwrap();
        let mut registry: TaskRegistry = serde_yaml::from_str(include_str!(
            "../../../flows/tasks/minimal-summary.tasks.yaml"
        ))
        .unwrap();
        let coder = registry.tasks["summarize-text"].clone();
        let mut child = plan.clone();
        child.nodes.get_mut("summarize").unwrap().task_ref = Some("reason".into());
        let mut reasoner = coder.clone();
        reasoner.model = "reasoner".into();
        registry.tasks.insert("reason".into(), reasoner);
        let mut unused = coder.clone();
        unused.model = "unused".into();
        registry.tasks.insert("unused-task".into(), unused);
        plan.subflows.insert("child".into(), child);
        assert_eq!(
            preparation::selected_slots(&plan, &registry),
            vec!["coder", "reasoner"]
        );
        let mut extra = coder;
        extra.model = "extra".into();
        registry.tasks.insert("extra-task".into(), extra);
        let mut extra_node = plan.nodes["summarize"].clone();
        extra_node.task_ref = Some("extra-task".into());
        plan.nodes.insert("extra".into(), extra_node);
        assert_eq!(
            preparation::selected_slots(&plan, &registry),
            Vec::<String>::new()
        );
    }

    #[test]
    fn preparation_drop_wakes_held_workers_and_loader_panic_is_retained() {
        for panics in [false, true] {
            let base = TrustTestDir::new("prepare-drop-panic");
            let path = base.0.join("weights.gguf");
            fs::write(&path, b"abc").unwrap();
            let (handle, _) = handle();
            let mut lazy = lazy_fixture(
                path,
                handle.control.clone(),
                Arc::new(AtomicU64::new(0)),
                Arc::new(AtomicU64::new(0)),
            );
            if panics {
                lazy.loader = Box::new(|_| panic!("synthetic native panic"));
            } else {
                handle.control.request_pause();
            }
            let mut preparations = Preparations::default();
            let mut model = preparations.add(lazy);
            preparations.start(Duration::from_secs(60)).unwrap();
            if panics {
                preparations.finish_for_test();
            }
            let (finished, received) = channel();
            let cleanup = std::thread::spawn(move || {
                drop(preparations);
                finished.send(()).unwrap();
            });
            received.recv_timeout(Duration::from_secs(2)).unwrap();
            cleanup.join().unwrap();
            handle.control.request_resume();
            let error = model.generate(request()).unwrap_err().to_string();
            if panics {
                assert!(error.contains("worker panicked"));
            } else {
                assert!(error.contains(CANCEL_MARKER));
            }
            assert!(handle
                .control
                .preparation_activity
                .lock()
                .unwrap()
                .is_empty());
            assert_eq!(handle.control.hold_ns.load(Ordering::Relaxed), 0);
        }
    }

    #[test]
    fn parallel_preparation_overlaps_reuses_and_reports_once() {
        let base = TrustTestDir::new("parallel-prepare");
        let path = base.0.join("weights.gguf");
        fs::write(&path, b"abc").unwrap();
        let (handle, _) = handle();
        handle.control.set_activity(None);
        let (entered_tx, entered_rx) = channel();
        let release = Arc::new((Mutex::new(false), Condvar::new()));
        let loads = Arc::new(AtomicU64::new(0));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let mut preparations = Preparations::default();
        let mut models = Vec::new();
        for slot in ["reasoner", "coder"] {
            let mut lazy = lazy_fixture(
                path.clone(),
                handle.control.clone(),
                loads.clone(),
                Arc::new(AtomicU64::new(0)),
            );
            lazy.slot = slot.into();
            let entered = entered_tx.clone();
            let release = release.clone();
            let requests = requests.clone();
            let loads = loads.clone();
            lazy.loader = Box::new(move |_| {
                loads.fetch_add(1, Ordering::SeqCst);
                entered.send(()).unwrap();
                let (gate, wake) = &*release;
                let mut ready = gate.lock().unwrap();
                while !*ready {
                    let (next, timed) = wake.wait_timeout(ready, Duration::from_secs(3)).unwrap();
                    ready = next;
                    if timed.timed_out() && !*ready {
                        bail!("synthetic load release timed out");
                    }
                }
                Ok(Box::new(BudgetRecordingModel(requests.clone())))
            });
            models.push(preparations.add(lazy));
        }
        preparations.start(Duration::from_secs(5)).unwrap();
        // Both loaders must enter before either is allowed to finish.
        entered_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        entered_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(loads.load(Ordering::SeqCst), 2);
        assert_eq!(
            handle.control.activity(),
            Some(RunActivityApi::LoadingModel {
                slot: "coder".into()
            })
        );
        handle.control.set_activity(Some(RunActivityApi::Starting));
        let foreground = handle.control.activity();
        {
            let (gate, wake) = &*release;
            *gate.lock().unwrap() = true;
            wake.notify_all();
        }
        preparations.finish_for_test();
        drop(preparations); // Removes only background activity.
        assert_eq!(handle.control.activity(), foreground);
        fs::write(&path, b"changed after verification").unwrap();
        for mut model in models {
            let first = model.generate(request()).unwrap().diagnostics.unwrap();
            assert_eq!(first.preparation_async, Some(true));
            assert!(first.preparation_wait_ms.is_some());
            assert!(first.weight_verify_ms.is_some());
            assert!(first.load_ms.is_some());
            let warm = model.generate(request()).unwrap().diagnostics.unwrap();
            assert!(warm.preparation_async.is_none());
            assert!(warm.load_ms.is_none());
        }
        assert_eq!(loads.load(Ordering::SeqCst), 2);
        assert_eq!(requests.lock().unwrap().len(), 4);
    }

    #[test]
    fn preparation_failure_is_deferred_and_retained_without_reload() {
        let base = TrustTestDir::new("prepare-bad-checksum");
        let path = base.0.join("weights.gguf");
        fs::write(&path, b"corrupt").unwrap();
        let (handle, _) = handle();
        handle.control.set_activity(None);
        let loads = Arc::new(AtomicU64::new(0));
        let lazy = lazy_fixture(
            path.clone(),
            handle.control.clone(),
            loads.clone(),
            Arc::new(AtomicU64::new(0)),
        );
        let mut preparations = Preparations::default();
        let mut model = preparations.add(lazy);
        preparations.start(Duration::from_secs(5)).unwrap();
        // Background failure by itself does not fail a run using another slot.
        preparations.finish_for_test();
        drop(preparations);
        assert_eq!(*handle.status.lock().unwrap(), RunPhase::Running);
        assert_eq!(loads.load(Ordering::SeqCst), 0);
        let error = model.generate(request()).unwrap_err();
        assert!(error.to_string().contains("mismatch"));
        let diagnostics = &error
            .downcast_ref::<trollm_models::ModelDiagnosticError>()
            .unwrap()
            .diagnostics;
        assert_eq!(diagnostics.preparation_async, Some(true));
        assert!(diagnostics.load_ms.is_none());
        fs::write(&path, b"abc").unwrap();
        assert!(model.generate(request()).is_err());
        assert_eq!(loads.load(Ordering::SeqCst), 0);
        // A reconstructed run owns a fresh verified slot; the failed slot above
        // stays failed even after the backing file is repaired.
        let reconstructed = lazy_fixture(
            path,
            handle.control.clone(),
            loads.clone(),
            Arc::new(AtomicU64::new(0)),
        );
        let mut next_run = Preparations::default();
        let mut recovered = next_run.add(reconstructed);
        next_run.start(Duration::from_secs(5)).unwrap();
        next_run.finish_for_test();
        assert_eq!(recovered.generate(request()).unwrap().text, "fake response");
        assert_eq!(loads.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn preparation_wait_uses_call_budget_and_cleanup_joins_native_load() {
        let base = TrustTestDir::new("prepare-wait-budget");
        let path = base.0.join("weights.gguf");
        fs::write(&path, b"abc").unwrap();
        let (handle, _) = handle();
        let (entered_tx, entered_rx) = channel();
        let (release_tx, release_rx) = channel();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let recorded = requests.clone();
        let mut lazy = lazy_fixture(
            path,
            handle.control.clone(),
            Arc::new(AtomicU64::new(0)),
            Arc::new(AtomicU64::new(0)),
        );
        lazy.loader = Box::new(move |_| {
            entered_tx.send(()).unwrap();
            release_rx
                .recv_timeout(Duration::from_secs(3))
                .context("synthetic load release")?;
            Ok(Box::new(BudgetRecordingModel(recorded.clone())))
        });
        let mut preparations = Preparations::default();
        let model = preparations.add(lazy);
        preparations.start(Duration::from_secs(5)).unwrap();
        entered_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        let mut controlled = ControlledModel {
            slot: "coder".into(),
            inner: model,
            ctl: handle.control.clone(),
        };
        let generator = std::thread::spawn(move || {
            let mut req = request();
            req.max_duration = Some(Duration::from_millis(100));
            controlled.generate(req).unwrap()
        });
        let started = Instant::now();
        while handle.control.activity()
            != Some(RunActivityApi::LoadingModel {
                slot: "coder".into(),
            })
        {
            assert!(started.elapsed() < Duration::from_secs(2));
            std::thread::yield_now();
        }
        let response = generator.join().unwrap();
        assert_eq!(response.finish, trollm_models::FinishReason::Timeout);
        let diagnostics = response.diagnostics.unwrap();
        assert_eq!(diagnostics.preparation_async, Some(true));
        assert!(diagnostics.preparation_wait_ms.unwrap() >= 90);
        assert!(requests.lock().unwrap().is_empty());
        let (dropped_tx, dropped_rx) = channel();
        // Selected completion must not treat foreground terminal phase as an
        // inactive worker while a native preparation load still owns the run.
        *handle.status.lock().unwrap() = RunPhase::Failed {
            reason: "foreground generation timed out".into(),
        };
        let worker_control = handle.control.clone();
        let cleanup = std::thread::spawn(move || {
            let worker = WorkerGuard(worker_control);
            drop(preparations);
            drop(worker);
            dropped_tx.send(()).unwrap();
        });
        assert!(dropped_rx.recv_timeout(Duration::from_millis(20)).is_err());
        assert!(handle.control.worker_active());
        release_tx.send(()).unwrap();
        dropped_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        cleanup.join().unwrap();
        assert!(!handle.control.worker_active());
        assert!(requests.lock().unwrap().is_empty());
        assert!(handle
            .control
            .preparation_activity
            .lock()
            .unwrap()
            .is_empty());
    }

    #[test]
    fn preparation_can_finish_after_a_returned_child_deadline() {
        use trollm_core::state::{ForeachCursor, ForeachPhase, StateStatus};
        use trollm_mcp::{ToolCall, ToolDescriptor, ToolGateway, ToolResult};

        struct NoTools;
        impl ToolGateway for NoTools {
            fn list_tools(&self) -> Result<Vec<ToolDescriptor>> {
                Ok(vec![])
            }
            fn tool_class(&self, _: &str, _: &str) -> trollm_mcp::policy::ToolClass {
                trollm_mcp::policy::ToolClass::Read
            }
            fn validate_call(&self, _: &ToolCall) -> Result<()> {
                bail!("unexpected tool")
            }
            fn call(&mut self, _: &ToolCall) -> Result<ToolResult> {
                bail!("unexpected tool")
            }
        }

        let base = TrustTestDir::new("prepare-after-child");
        fs::write(base.0.join("reason.md"), "Give a brief answer.").unwrap();
        let mut root = ExecutionPlan::from_yaml_str(
            r#"
schemaVersion: ledger.execution-plan.v1
id: root
version: 1
input: { promptTemplate: "" }
limits: { maxSteps: 10, maxReasonerCalls: 1, maxTokensTotal: 1000, timeoutS: 60, artifactMaxMb: 1 }
onLimit: abort
models: { reasoner: { ctx: 4096 } }
mcp: { servers: [], policies: {} }
entryStep: each
nodes:
  each:
    kind: foreach
    items: { from: items }
    itemKey: id
    bindAs: item
    enumeration: snapshot
    ordering: source
    body: { flowRef: child }
    outputs: { results: { collect: body.outputs, indexBy: itemKey } }
    onEmpty: reason
    onComplete: reason
  reason: { kind: task, taskRef: answer, onVerified: done }
  done: { kind: end, outcome: succeeded }
"#,
        )
        .unwrap();
        let child = ExecutionPlan::from_yaml_str(
            r#"
schemaVersion: ledger.execution-plan.v1
id: child
version: 1
input: { promptTemplate: "" }
limits: { maxSteps: 2, maxReasonerCalls: 0, maxTokensTotal: 1000, timeoutS: 3, artifactMaxMb: 1 }
onLimit: abort
models: { reasoner: { ctx: 4096 } }
mcp: { servers: [], policies: {} }
entryStep: done
nodes:
  done: { kind: end, outcome: succeeded }
"#,
        )
        .unwrap();
        let child_pin = child.pin();
        root.subflows.insert("child".into(), child);
        let registry = TaskRegistry::from_yaml_str("schemaVersion: ledger.task-registry.v1\ntasks:\n  answer:\n    model: reasoner\n    prompt: reason.md\n    tools: []\n    outputContract: { outputs: { report: { from: model, type: prose } } }", &base.0).unwrap();
        let run = RunDir::under(&base.0);
        run.create_dirs().unwrap();
        fs::write(
            run.artifacts_dir().join("each.snapshot.jsonl"),
            "{\"id\":1}\n",
        )
        .unwrap();
        let mut state = trollm_core::RunState::fresh(run.run_id(), &root.pin(), "each");
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        state.stack[0].counters.started_at = now - 5_000;
        let mut child_frame = trollm_core::RunState::fresh(run.run_id(), &child_pin, "done")
            .stack
            .remove(0);
        child_frame.counters.started_at = now;
        let child_deadline_ms = now + 3_000;
        state.stack.push(child_frame);
        state.status = StateStatus::Paused;
        state.foreach_cursors.insert(
            "each".into(),
            ForeachCursor {
                index: 0,
                snapshot_ref: "artifacts/each.snapshot.jsonl".into(),
                session_items: 0,
                results: vec![],
                phase: ForeachPhase::InBody,
            },
        );
        fs::write(run.snapshot_path(), serde_json::to_vec(&state).unwrap()).unwrap();

        let path = base.0.join("weights.gguf");
        fs::write(&path, b"abc").unwrap();
        let (handle, _) = handle();
        let (entered_tx, entered_rx) = channel();
        let (release_tx, release_rx) = channel();
        let (parent_tx, parent_rx) = channel();
        let loads = Arc::new(AtomicU64::new(0));
        let mut lazy = lazy_fixture(
            path,
            handle.control.clone(),
            Arc::new(AtomicU64::new(0)),
            Arc::new(AtomicU64::new(0)),
        );
        let requests = Arc::new(Mutex::new(Vec::new()));
        let recorded = requests.clone();
        let load_count = loads.clone();
        lazy.loader = Box::new(move |_| {
            load_count.fetch_add(1, Ordering::SeqCst);
            entered_tx.send(()).unwrap();
            release_rx
                .recv_timeout(Duration::from_secs(10))
                .context("synthetic load release")?;
            Ok(Box::new(BudgetRecordingModel(recorded.clone())))
        });
        let mut preparations = Preparations::default();
        let model = ControlledModel {
            slot: "reasoner".into(),
            inner: preparations.add(lazy),
            ctl: handle.control.clone(),
        };
        let mut exec = Executor::new(
            root,
            registry,
            String::new(),
            SlotMap::new().with("reasoner", model),
            NoTools,
            run,
        )
        .unwrap();
        exec.set_control(handle.control.clone());
        exec.set_event_sink(Arc::new(move |event, _| {
            if event["type"] == "node_entered" && event["node"] == "reason" {
                let _ = parent_tx.send(());
            }
        }));
        assert!(exec.remaining_duration() > Duration::from_secs(1));
        assert!(exec.remaining_duration() <= Duration::from_secs(3));
        assert!(exec.remaining_run_duration() > Duration::from_secs(50));
        start_run_preparations(&mut preparations, &exec).unwrap();
        entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let release = std::thread::spawn(move || {
            parent_rx.recv_timeout(Duration::from_secs(5)).unwrap();
            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64;
            std::thread::sleep(Duration::from_millis(
                child_deadline_ms.saturating_sub(now_ms) + 50,
            ));
            release_tx.send(()).unwrap();
        });
        let outcome = exec.resume().unwrap();
        release.join().unwrap();
        preparations.finish_for_test();
        assert_eq!(outcome, RunStatus::Succeeded);
        assert_eq!(loads.load(Ordering::SeqCst), 1);
        assert_eq!(requests.lock().unwrap().len(), 1);
    }

    #[test]
    fn preparation_pause_deadline_and_cancel_do_not_count_hold_or_change_phase() {
        for cancel in [false, true] {
            let base = TrustTestDir::new("prepare-controls");
            let path = base.0.join("weights.gguf");
            fs::write(&path, b"abc").unwrap();
            let (handle, _) = handle();
            handle.control.request_pause();
            let loads = Arc::new(AtomicU64::new(0));
            let lazy = lazy_fixture(
                path,
                handle.control.clone(),
                loads.clone(),
                Arc::new(AtomicU64::new(0)),
            );
            let mut preparations = Preparations::default();
            let _model = preparations.add(lazy);
            preparations.start(Duration::from_millis(20)).unwrap();
            if cancel {
                handle.control.request_cancel();
            }
            preparations.finish_for_test();
            drop(preparations);
            assert_eq!(loads.load(Ordering::SeqCst), 0);
            assert_eq!(handle.control.hold_ns.load(Ordering::Relaxed), 0);
            assert_eq!(*handle.status.lock().unwrap(), RunPhase::Running);
            assert!(handle
                .control
                .preparation_activity
                .lock()
                .unwrap()
                .is_empty());
        }
    }

    struct BudgetRecordingModel(Arc<Mutex<Vec<Option<Duration>>>>);

    impl ModelSlot for BudgetRecordingModel {
        fn generate(&mut self, req: GenerateRequest) -> Result<GenerateResponse> {
            self.0.lock().unwrap().push(req.max_duration);
            Ok(GenerateResponse {
                text: "partial evidence".into(),
                tokens_in: 1,
                tokens_out: 1,
                finish: trollm_models::FinishReason::Stop,
                capacity: None,
                diagnostics: None,
            })
        }
    }

    #[test]
    fn lazy_loading_consumes_generation_allowance_and_does_not_generate_after_expiry() {
        let base = TrustTestDir::new("lazy-budget");
        let path = base.0.join("weights.gguf");
        fs::write(&path, b"abc").unwrap();
        let (handle, _) = handle();
        let mut lazy = lazy_fixture(
            path,
            handle.control.clone(),
            Arc::new(AtomicU64::new(0)),
            Arc::new(AtomicU64::new(0)),
        );
        let requests = Arc::new(Mutex::new(Vec::new()));
        let recorded = requests.clone();
        lazy.loader = Box::new(move |_| {
            std::thread::sleep(Duration::from_millis(30));
            Ok(Box::new(BudgetRecordingModel(recorded.clone())))
        });
        let mut req = request();
        req.max_duration = Some(Duration::from_millis(10));
        let response = lazy.generate(req).unwrap();
        assert_eq!(response.finish, trollm_models::FinishReason::Timeout);
        assert!(response.text.is_empty());
        let diagnostics = response.diagnostics.unwrap();
        assert!(diagnostics.weight_verify_ms.is_some());
        assert!(diagnostics.load_ms.is_some());
        assert!(diagnostics.prepare_ms.is_none());
        assert!(diagnostics.prefill_ms.is_none());
        assert!(diagnostics.generation_ms.is_none());
        assert!(requests.lock().unwrap().is_empty());
        assert!(lazy.loaded.is_some(), "a completed load remains reusable");
        assert_eq!(handle.control.activity(), None);
        let warm = lazy.generate(request()).unwrap();
        let diagnostics = warm.diagnostics.unwrap();
        assert!(diagnostics.weight_verify_ms.is_none());
        assert!(diagnostics.load_ms.is_none());
        assert_eq!(*requests.lock().unwrap(), vec![None]);
    }

    #[test]
    fn lazy_loading_forwards_only_remaining_duration() {
        let base = TrustTestDir::new("lazy-budget-remaining");
        let path = base.0.join("weights.gguf");
        fs::write(&path, b"abc").unwrap();
        let (handle, _) = handle();
        let mut lazy = lazy_fixture(
            path,
            handle.control.clone(),
            Arc::new(AtomicU64::new(0)),
            Arc::new(AtomicU64::new(0)),
        );
        let requests = Arc::new(Mutex::new(Vec::new()));
        let recorded = requests.clone();
        lazy.loader = Box::new(move |_| {
            std::thread::sleep(Duration::from_millis(20));
            Ok(Box::new(BudgetRecordingModel(recorded.clone())))
        });
        let mut req = request();
        req.max_duration = Some(Duration::from_secs(5));
        lazy.generate(req).unwrap();
        let remaining = requests.lock().unwrap()[0].unwrap();
        assert!(remaining <= Duration::from_millis(4980));
        assert!(!remaining.is_zero());
    }

    struct PauseAfterResponse {
        ctl: Arc<RunControl>,
        ready: Sender<()>,
    }

    impl ModelSlot for PauseAfterResponse {
        fn generate(&mut self, _: GenerateRequest) -> Result<GenerateResponse> {
            self.ctl.request_pause();
            self.ready.send(()).unwrap();
            Ok(GenerateResponse {
                text: "partial evidence".into(),
                tokens_in: 3,
                tokens_out: 2,
                finish: trollm_models::FinishReason::Stop,
                capacity: None,
                diagnostics: None,
            })
        }
    }

    #[test]
    fn controlled_model_preserves_response_when_post_generation_pause_exhausts_budget() {
        let (handle, _) = handle();
        let (ready, waiting) = channel();
        let mut model = ControlledModel {
            slot: "coder".into(),
            inner: Box::new(PauseAfterResponse {
                ctl: handle.control.clone(),
                ready,
            }),
            ctl: handle.control.clone(),
        };
        let worker = std::thread::spawn(move || {
            let mut req = request();
            req.max_duration = Some(Duration::from_millis(10));
            model.generate(req)
        });
        waiting.recv_timeout(Duration::from_secs(2)).unwrap();
        std::thread::sleep(Duration::from_millis(20));
        handle.control.request_resume();
        let response = worker.join().unwrap().unwrap();
        assert_eq!(response.finish, trollm_models::FinishReason::Timeout);
        assert_eq!(response.text, "partial evidence");
        assert_eq!((response.tokens_in, response.tokens_out), (3, 2));
        assert_eq!(handle.control.activity(), None);
    }

    #[test]
    fn controlled_model_counts_paused_time_before_dispatch() {
        let (handle, _) = handle();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let mut model = ControlledModel {
            slot: "coder".into(),
            inner: Box::new(BudgetRecordingModel(requests.clone())),
            ctl: handle.control.clone(),
        };
        handle.control.request_pause();
        let worker = std::thread::spawn(move || {
            let mut req = request();
            req.max_duration = Some(Duration::from_millis(10));
            model.generate(req)
        });
        let wait_started = Instant::now();
        while !matches!(*handle.status.lock().unwrap(), RunPhase::Paused { .. }) {
            assert!(wait_started.elapsed() < Duration::from_secs(2));
            std::thread::sleep(Duration::from_millis(1));
        }
        std::thread::sleep(Duration::from_millis(20));
        handle.control.request_resume();
        let response = worker.join().unwrap().unwrap();
        assert_eq!(response.finish, trollm_models::FinishReason::Timeout);
        assert!(requests.lock().unwrap().is_empty());
        assert_eq!(handle.control.activity(), None);
    }

    #[test]
    fn lazy_model_verifies_first_use_reuses_and_reverifies_reconstruction() {
        let base = TrustTestDir::new("lazy-model");
        let path = base.0.join("weights.gguf");
        let loads = Arc::new(AtomicU64::new(0));
        let dropped = Arc::new(AtomicU64::new(0));
        let (handle, _) = handle();
        let mut lazy = lazy_fixture(
            path.clone(),
            handle.control.clone(),
            loads.clone(),
            dropped.clone(),
        );
        // Construction requires neither a file nor a native loader.
        assert_eq!(loads.load(Ordering::SeqCst), 0);
        assert!(lazy.loaded.is_none());
        fs::write(&path, b"abc").unwrap();
        assert_eq!(lazy.generate(request()).unwrap().text, "fake response");
        assert_eq!(loads.load(Ordering::SeqCst), 1);
        assert_eq!(handle.control.activity(), None);
        // Reuse does not re-open/re-hash the model until a new executor owns it.
        fs::write(&path, b"corrupt").unwrap();
        lazy.generate(request()).unwrap();
        assert_eq!(loads.load(Ordering::SeqCst), 1);
        drop(lazy);
        assert_eq!(dropped.load(Ordering::SeqCst), 1);
        let mut reconstructed =
            lazy_fixture(path.clone(), handle.control.clone(), loads.clone(), dropped);
        assert!(reconstructed
            .generate(request())
            .unwrap_err()
            .to_string()
            .contains("mismatch"));
        assert_eq!(loads.load(Ordering::SeqCst), 1);
        assert_eq!(handle.control.activity(), None);
        fs::remove_file(path).unwrap();
        assert!(reconstructed
            .generate(request())
            .unwrap_err()
            .to_string()
            .contains("no weights"));
        assert_eq!(loads.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn failed_loader_preserves_error_and_reverifies_on_retry() {
        let base = TrustTestDir::new("lazy-load-retry");
        let path = base.0.join("weights.gguf");
        fs::write(&path, b"abc").unwrap();
        let (handle, _) = handle();
        let loads = Arc::new(AtomicU64::new(0));
        let mut lazy = lazy_fixture(
            path.clone(),
            handle.control.clone(),
            loads.clone(),
            Arc::new(AtomicU64::new(0)),
        );
        let attempts = loads.clone();
        lazy.loader = Box::new(move |_| {
            attempts.fetch_add(1, Ordering::SeqCst);
            bail!("native allocation failed; free memory and retry");
        });
        let error = lazy.generate(request()).unwrap_err();
        assert!(error.to_string().contains("native allocation failed"));
        let diagnostics = &error
            .downcast_ref::<trollm_models::ModelDiagnosticError>()
            .unwrap()
            .diagnostics;
        assert!(diagnostics.weight_verify_ms.is_some());
        assert!(diagnostics.load_ms.is_some());
        assert!(diagnostics.prepare_ms.is_none());
        assert!(diagnostics.prefill_ms.is_none());
        assert_eq!(loads.load(Ordering::SeqCst), 1);
        assert_eq!(handle.control.activity(), None);
        fs::write(&path, b"corrupt").unwrap();
        assert!(lazy
            .generate(request())
            .unwrap_err()
            .to_string()
            .contains("mismatch"));
        assert_eq!(loads.load(Ordering::SeqCst), 1);
        fs::write(path, b"abc").unwrap();
        assert!(lazy
            .generate(request())
            .unwrap_err()
            .to_string()
            .contains("native allocation failed"));
        assert_eq!(loads.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn worker_guard_clears_ephemeral_state_on_unwind() {
        let (handle, _) = handle();
        let ctl = handle.control.clone();
        assert!(std::thread::spawn(move || {
            let _guard = WorkerGuard(ctl);
            panic!("synthetic worker panic");
        })
        .join()
        .is_err());
        assert!(!handle.control.worker_active());
        assert_eq!(handle.control.activity(), None);
        let registry = RunRegistry::default();
        registry.insert("abandoned".into(), handle);
        assert!(!registry.has_active());
    }

    #[test]
    fn controlled_model_reports_generating_and_clears_errors() {
        let (handle, _) = handle();
        let dropped = Arc::new(AtomicU64::new(0));
        let mut model = ControlledModel {
            slot: "coder".into(),
            inner: Box::new(RecordingModel {
                ctl: handle.control.clone(),
                dropped,
            }),
            ctl: handle.control.clone(),
        };
        model.generate(request()).unwrap();
        assert_eq!(handle.control.activity(), None);
        // An ordinary model error follows the same guard cleanup path.
        model.inner = Box::new(trollm_models::MockModel::default());
        assert!(model.generate(request()).is_err());
        assert_eq!(handle.control.activity(), None);
    }

    #[test]
    fn cancelled_lazy_model_never_loads() {
        let base = TrustTestDir::new("cancelled-lazy");
        let path = base.0.join("weights.gguf");
        fs::write(&path, b"abc").unwrap();
        let (handle, _) = handle();
        let loads = Arc::new(AtomicU64::new(0));
        let mut lazy = lazy_fixture(
            path,
            handle.control.clone(),
            loads.clone(),
            Arc::new(AtomicU64::new(0)),
        );
        handle.cancel();
        assert_eq!(
            lazy.generate(request()).unwrap_err().to_string(),
            CANCEL_MARKER
        );
        assert_eq!(loads.load(Ordering::SeqCst), 0);
        assert_eq!(handle.control.activity(), None);
    }

    #[test]
    fn status_reopen_is_read_only_and_startup_interruption_is_explicit() {
        const INTERRUPTED_ID: &str = "00000000-0000-0000-0000-000000000001";
        const SAVED_ID: &str = "00000000-0000-0000-0000-000000000002";
        use crate::{RunPhaseApi, TrollmRuntime};
        let base = TrustTestDir::new("status-reopen");
        let dir = base.0.join("trollm/runs").join(INTERRUPTED_ID);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("events.jsonl"), b"").unwrap();
        let rt = TrollmRuntime::new(base.0.to_string_lossy().into_owned(), None).unwrap();
        let interrupted = rt.poll_status(INTERRUPTED_ID.into()).unwrap();
        assert!(
            matches!(interrupted.phase, RunPhaseApi::Failed { reason } if reason.contains("interrupted before saved progress; start a new run"))
        );
        assert!(!interrupted.worker_active);
        assert_eq!(interrupted.activity, None);
        assert!(!dir.join("state.snapshot.json").exists());
        assert!(rt.resume(INTERRUPTED_ID.into()).is_err());
        assert_eq!(fs::read(dir.join("events.jsonl")).unwrap(), b"");
        let (abandoned, _) = handle();
        abandoned
            .control
            .worker_active
            .store(false, Ordering::SeqCst);
        rt.runs.insert(INTERRUPTED_ID.into(), abandoned);
        let interrupted = rt.poll_status(INTERRUPTED_ID.into()).unwrap();
        assert!(
            matches!(interrupted.phase, RunPhaseApi::Failed { reason } if reason.contains("interrupted before saved progress; start a new run"))
        );
        assert!(!interrupted.worker_active);
        assert_eq!(interrupted.activity, None);

        for state_status in [
            trollm_core::state::StateStatus::Running,
            trollm_core::state::StateStatus::Paused,
        ] {
            let mut state =
                trollm_core::RunState::fresh(SAVED_ID, "missing@sha256:absent", "entry");
            state.status = state_status;
            let saved = base.0.join("trollm/runs").join(SAVED_ID);
            fs::create_dir_all(&saved).unwrap();
            let bytes = serde_json::to_vec(&state).unwrap();
            fs::write(saved.join("state.snapshot.json"), &bytes).unwrap();
            let status = rt.poll_status(SAVED_ID.into()).unwrap();
            assert!(!status.worker_active);
            assert_eq!(status.activity, None);
            assert!(!rt.has_active_run());
            assert_eq!(fs::read(saved.join("state.snapshot.json")).unwrap(), bytes);
            assert!(!saved.join("events.jsonl").exists());
        }
    }

    #[test]
    fn concurrent_resume_reconstructs_one_worker_for_the_same_run() {
        const ID: &str = "00000000-0000-0000-0000-000000000001";
        let base = TrustTestDir::new("concurrent-resume");
        let rt = crate::TrollmRuntime::new(base.0.to_string_lossy().into_owned(), None).unwrap();
        let constructions = Arc::new(AtomicU64::new(0));
        let (entered, first_entered) = channel();
        let (release, released) = channel();
        let (attempted, second_attempted) = channel();
        let (finished, second_finished) = channel();
        let first_runtime = rt.clone();
        let first_count = constructions.clone();
        let first = std::thread::spawn(move || {
            first_runtime.resume_or_reconstruct(ID.into(), || {
                first_count.fetch_add(1, Ordering::SeqCst);
                entered.send(()).unwrap();
                released.recv_timeout(Duration::from_secs(2)).unwrap();
                Ok(handle().0)
            })
        });
        first_entered.recv_timeout(Duration::from_secs(2)).unwrap();
        let second_runtime = rt.clone();
        let second_count = constructions.clone();
        let second = std::thread::spawn(move || {
            attempted.send(()).unwrap();
            let result = second_runtime.resume_or_reconstruct(ID.into(), || {
                second_count.fetch_add(1, Ordering::SeqCst);
                Ok(handle().0)
            });
            finished.send(()).unwrap();
            result
        });
        second_attempted
            .recv_timeout(Duration::from_secs(2))
            .unwrap();
        // The first reconstruction is intentionally held before insertion.
        // A second request must wait, then observe that first registered worker.
        let blocked = matches!(
            second_finished.recv_timeout(Duration::from_millis(50)),
            Err(RecvTimeoutError::Timeout)
        );
        release.send(()).unwrap();
        first.join().unwrap().unwrap();
        second.join().unwrap().unwrap();
        assert!(
            blocked,
            "concurrent resume bypassed reconstruction serialization"
        );
        assert_eq!(constructions.load(Ordering::SeqCst), 1);
        assert!(rt.poll_status(ID.into()).unwrap().worker_active);
    }

    #[test]
    fn status_capture_excludes_terminal_publication_until_activity_is_read() {
        const ID: &str = "00000000-0000-0000-0000-000000000001";
        let base = TrustTestDir::new("status-consistency");
        let rt = crate::TrollmRuntime::new(base.0.to_string_lossy().into_owned(), None).unwrap();
        let (handle, _) = handle();
        rt.runs.insert(ID.into(), handle.clone());
        // Hold the final status component so the reader must pause mid-capture.
        let activity = handle.control.activity.lock().unwrap();
        let (started, reader_started) = channel();
        let reader = std::thread::spawn(move || {
            started.send(()).unwrap();
            rt.poll_status(ID.into()).unwrap()
        });
        reader_started.recv_timeout(Duration::from_secs(2)).unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        let phase_is_held = loop {
            if handle.status.try_lock().is_err() {
                break true;
            }
            if Instant::now() >= deadline {
                break false;
            }
            std::thread::yield_now();
        };
        // Cleanup before asserting so a failing regression never strands a reader.
        drop(activity);
        let status = reader.join().unwrap();
        assert!(
            phase_is_held,
            "terminal publication could race activity capture"
        );
        assert!(matches!(status.phase, crate::RunPhaseApi::Running));
        assert!(status.worker_active);
        assert_eq!(status.activity, Some(RunActivityApi::Starting));
    }

    #[test]
    fn status_only_exposes_activity_for_active_running_worker() {
        const ID: &str = "00000000-0000-0000-0000-000000000001";
        let base = TrustTestDir::new("active-status");
        let rt = crate::TrollmRuntime::new(base.0.to_string_lossy().into_owned(), None).unwrap();
        let (handle, _) = handle();
        rt.runs.insert(ID.into(), handle.clone());
        let status = rt.poll_status(ID.into()).unwrap();
        assert!(status.worker_active);
        assert!(rt
            .prepare_source_report(ID.into(), "controlled-device".into())
            .unwrap_err()
            .to_string()
            .contains("worker stops"));
        assert_eq!(status.activity, Some(RunActivityApi::Starting));
        *handle.status.lock().unwrap() = RunPhase::Paused {
            reason: "hold".into(),
        };
        let status = rt.poll_status(ID.into()).unwrap();
        assert!(status.worker_active);
        assert!(rt
            .prepare_source_report(ID.into(), "controlled-device".into())
            .unwrap_err()
            .to_string()
            .contains("worker stops"));
        assert_eq!(status.activity, None);
        handle.resume();
        assert!(matches!(*handle.status.lock().unwrap(), RunPhase::Running));
        *handle.status.lock().unwrap() = RunPhase::Succeeded;
        let status = rt.poll_status(ID.into()).unwrap();
        assert!(
            status.worker_active,
            "terminal phase is not worker-stop proof"
        );
        assert!(rt
            .prepare_source_report(ID.into(), "controlled-device".into())
            .unwrap_err()
            .to_string()
            .contains("worker stops"));
        assert_eq!(status.activity, None);
        drop(WorkerGuard(handle.control.clone()));
        let status = rt.poll_status(ID.into()).unwrap();
        assert!(!status.worker_active);
        assert_eq!(status.activity, None);
    }

    #[test]
    fn cooperative_hold_stops_boundary_until_resume_or_cancel() {
        for cancel in [false, true] {
            let (handle, commands) = handle();
            handle.pause();
            let ctl = handle.control.clone();
            let (done, result) = channel();
            let worker = std::thread::spawn(move || done.send(ctl.before_boundary()).unwrap());
            let deadline = Instant::now() + Duration::from_secs(2);
            while !matches!(*handle.status.lock().unwrap(), RunPhase::Paused { .. }) {
                assert!(
                    Instant::now() < deadline,
                    "worker never acknowledged the hold"
                );
                std::thread::yield_now();
            }
            // The worker acknowledged the pause and must not cross this boundary.
            assert!(matches!(
                result.recv_timeout(Duration::from_millis(50)),
                Err(RecvTimeoutError::Timeout)
            ));
            if cancel {
                handle.cancel();
            } else {
                handle.resume();
            }
            let outcome = result
                .recv_timeout(Duration::from_secs(2))
                .expect("lost wakeup");
            if cancel {
                assert_eq!(outcome.unwrap_err().to_string(), CANCEL_MARKER);
            } else {
                outcome.unwrap();
                assert!(
                    commands.try_recv().is_err(),
                    "user resume queued a future plan command"
                );
            }
            worker.join().unwrap();
        }
    }

    #[test]
    fn deadline_backoff_expires_during_pause_without_waiting_again_after_resume() {
        let (handle, _) = handle();
        handle.pause();
        let ctl = handle.control.clone();
        let (deadline_sender, deadline_receiver) = channel();
        let (done, result) = channel();
        let worker = std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_millis(200);
            deadline_sender.send(deadline).unwrap();
            done.send(ctl.wait_backoff_until(Duration::from_secs(30), Some(deadline)))
                .unwrap();
        });
        let deadline = deadline_receiver
            .recv_timeout(Duration::from_secs(2))
            .unwrap();
        let acknowledged_by = Instant::now() + Duration::from_secs(2);
        while !matches!(*handle.status.lock().unwrap(), RunPhase::Paused { .. }) {
            assert!(
                Instant::now() < acknowledged_by,
                "backoff did not acknowledge pause"
            );
            std::thread::yield_now();
        }
        // Expiry does not implicitly release an explicit user hold.
        assert!(matches!(
            result.recv_timeout(
                deadline.saturating_duration_since(Instant::now()) + Duration::from_millis(10)
            ),
            Err(RecvTimeoutError::Timeout)
        ));
        handle.resume();
        let completion = result.recv_timeout(Duration::from_secs(2));
        if completion.is_err() {
            handle.cancel();
        }
        worker.join().unwrap();
        assert!(!completion.expect("expired deadline resumed a fresh backoff wait"));
    }

    #[test]
    fn backoff_wait_wakes_for_cancel_and_freezes_during_pause() {
        let (cancel_handle, _) = handle();
        let ctl = cancel_handle.control.clone();
        let (started, ready) = channel();
        let (done, result) = channel();
        let worker = std::thread::spawn(move || {
            started.send(()).unwrap();
            done.send(ctl.wait_backoff(Duration::from_secs(u64::MAX)))
                .unwrap();
        });
        ready.recv_timeout(Duration::from_secs(2)).unwrap();
        cancel_handle.pause();
        let deadline = Instant::now() + Duration::from_secs(2);
        while !matches!(
            *cancel_handle.status.lock().unwrap(),
            RunPhase::Paused { .. }
        ) {
            assert!(
                Instant::now() < deadline,
                "backoff did not acknowledge pause"
            );
            std::thread::yield_now();
        }
        assert!(matches!(
            result.recv_timeout(Duration::from_millis(50)),
            Err(RecvTimeoutError::Timeout)
        ));
        cancel_handle.resume();
        cancel_handle.cancel();
        assert!(result.recv_timeout(Duration::from_secs(2)).unwrap());
        worker.join().unwrap();

        let (handle, _) = handle();
        handle.pause();
        let ctl = handle.control.clone();
        let (done, result) = channel();
        let worker = std::thread::spawn(move || {
            done.send(ctl.wait_backoff(Duration::from_millis(200)))
                .unwrap();
        });
        let deadline = Instant::now() + Duration::from_secs(2);
        while !matches!(*handle.status.lock().unwrap(), RunPhase::Paused { .. }) {
            assert!(
                Instant::now() < deadline,
                "backoff did not acknowledge pause"
            );
            std::thread::yield_now();
        }
        assert!(matches!(
            result.recv_timeout(Duration::from_millis(250)),
            Err(RecvTimeoutError::Timeout)
        ));
        handle.resume();
        assert!(matches!(
            result.recv_timeout(Duration::from_millis(50)),
            Err(RecvTimeoutError::Timeout)
        ));
        assert!(!result.recv_timeout(Duration::from_secs(2)).unwrap());
        worker.join().unwrap();
    }

    #[test]
    fn cold_status_reads_terminal_reason_and_legacy_failure() {
        let base = TrustTestDir::new("terminal-reason");
        let run_dir = base.0.join("runs").join("old");
        fs::create_dir_all(&run_dir).unwrap();
        let mut state = trollm_core::RunState::fresh("old", "plan@sha256:old", "work");
        state.status = trollm_core::state::StateStatus::Failed;
        state.terminal_reason = Some("cancelled".into());
        let mut value = serde_json::to_value(&state).unwrap();
        fs::write(
            run_dir.join("state.snapshot.json"),
            serde_json::to_vec(&value).unwrap(),
        )
        .unwrap();
        assert_eq!(
            status_from_disk(&base.0, "old").unwrap(),
            Some(RunPhase::Failed {
                reason: "cancelled".into()
            })
        );
        value.as_object_mut().unwrap().remove("terminalReason");
        fs::write(
            run_dir.join("state.snapshot.json"),
            serde_json::to_vec(&value).unwrap(),
        )
        .unwrap();
        assert_eq!(
            status_from_disk(&base.0, "old").unwrap(),
            Some(RunPhase::Failed {
                reason: "failed (persisted)".into()
            })
        );
    }

    #[test]
    fn notifiers_synchronize_with_the_wait_predicate() {
        for cancel in [false, true] {
            let (handle, _) = handle();
            let guard = handle.control.gate.lock().unwrap();
            let ctl = handle.control.clone();
            let (started, ready) = channel();
            let (done, result) = channel();
            let notifier = std::thread::spawn(move || {
                started.send(()).unwrap();
                if cancel {
                    ctl.request_cancel();
                } else {
                    ctl.request_resume();
                }
                done.send(()).unwrap();
            });
            ready.recv_timeout(Duration::from_secs(2)).unwrap();
            assert!(
                matches!(
                    result.recv_timeout(Duration::from_millis(50)),
                    Err(RecvTimeoutError::Timeout)
                ),
                "notifier changed the predicate outside the wait mutex"
            );
            drop(guard);
            result.recv_timeout(Duration::from_secs(2)).unwrap();
            notifier.join().unwrap();
        }
    }

    #[test]
    fn running_and_duplicate_resume_do_not_open_a_future_plan_pause() {
        let (handle, commands) = handle();
        handle.resume();
        handle.resume();
        assert!(commands.try_recv().is_err());
        {
            let mut gate = handle.control.gate.lock().unwrap();
            gate.plan_paused = true;
            *handle.status.lock().unwrap() = RunPhase::Paused {
                reason: "item limit".into(),
            };
        }
        assert!(
            commands.try_recv().is_err(),
            "old resume opened the new pause"
        );
        handle.resume();
        handle.resume();
        assert!(matches!(commands.try_recv(), Ok(RunCommand::Resume)));
        assert!(commands.try_recv().is_err(), "duplicate resume buffered");
    }
}
