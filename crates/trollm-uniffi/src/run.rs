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
#[cfg(test)]
use std::sync::atomic::AtomicU64;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};
use trollm_core::exec::{ExecutionControl, SlotMap};
use trollm_core::{EventType, ExecutionPlan, Executor, RunDir, RunStatus, TaskRegistry};
use trollm_mcp::{EventSink, McpGateway, PolicySet, ProxyEvent, RootCertificates, ServerConfig};
use trollm_models::{GenerateRequest, GenerateResponse, ModelSlot};

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
    cancel: AtomicBool,
    worker_active: AtomicBool,
    /// Keep the only available failure diagnostic if its sidecar could not be written.
    volatile_worker_failure: AtomicBool,
    activity: Mutex<Option<RunActivityApi>>,
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

impl RunControl {
    pub fn worker_active(&self) -> bool {
        self.worker_active.load(Ordering::SeqCst)
    }

    pub(crate) fn has_volatile_failure(&self) -> bool {
        self.volatile_worker_failure.load(Ordering::SeqCst)
    }

    pub fn activity(&self) -> Option<RunActivityApi> {
        self.activity.lock().unwrap().clone()
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
        if self.cancel.load(Ordering::SeqCst) {
            bail!(CANCEL_MARKER);
        }
        if self.pause_requested.load(Ordering::SeqCst) {
            {
                let mut status = self.status.lock().unwrap();
                *status = RunPhase::Paused {
                    reason: "user-requested (cooperative hold; not a persisted §4.4 pause)"
                        .to_string(),
                };
            }
            while self.pause_requested.load(Ordering::SeqCst) && !self.cancel.load(Ordering::SeqCst)
            {
                guard = self
                    .gate_cond
                    .wait(guard)
                    .map_err(|_| anyhow!("run control lock poisoned"))?;
            }
            *self.status.lock().unwrap() = RunPhase::Running;
        }
        if self.cancel.load(Ordering::SeqCst) {
            bail!(CANCEL_MARKER);
        }
        Ok(())
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
        self.cancel.store(true, Ordering::SeqCst);
        self.pause_requested.store(false, Ordering::SeqCst);
        self.gate_cond.notify_all();
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancel.load(Ordering::SeqCst)
    }
}

impl ExecutionControl for RunControl {
    fn checkpoint(&self) -> bool {
        self.before_boundary().is_err()
    }

    fn wait_backoff(&self, duration: Duration) -> bool {
        let mut remaining = duration;
        let mut guard = self.gate.lock().unwrap();
        loop {
            if self.cancel.load(Ordering::SeqCst) {
                return true;
            }
            if self.pause_requested.load(Ordering::SeqCst) {
                *self.status.lock().unwrap() = RunPhase::Paused {
                    reason: "user-requested (cooperative hold; not a persisted §4.4 pause)"
                        .to_string(),
                };
                while self.pause_requested.load(Ordering::SeqCst)
                    && !self.cancel.load(Ordering::SeqCst)
                {
                    guard = self.gate_cond.wait(guard).unwrap();
                }
                *self.status.lock().unwrap() = RunPhase::Running;
                continue;
            }
            if remaining.is_zero() {
                return false;
            }
            // Bounded chunks avoid `Instant + Duration` overflow for an
            // authored u64::MAX seconds and allow periodic predicate checks.
            let chunk = remaining.min(Duration::from_secs(60));
            let started = Instant::now();
            let (next, _) = self.gate_cond.wait_timeout(guard, chunk).unwrap();
            guard = next;
            remaining = remaining.saturating_sub(started.elapsed());
        }
    }
}

/// ModelSlot wrapper injecting pause/cancel checks around the real slot.
pub struct ControlledModel {
    slot: String,
    inner: Box<dyn ModelSlot>,
    ctl: Arc<RunControl>,
}

impl ModelSlot for ControlledModel {
    fn generate(&mut self, req: GenerateRequest) -> Result<GenerateResponse> {
        self.ctl.before_boundary()?;
        self.ctl.set_activity(Some(RunActivityApi::Generating {
            slot: self.slot.clone(),
        }));
        let _activity = ActivityGuard(self.ctl.clone());
        let resp = self.inner.generate(req)?;
        self.ctl.before_boundary()?;
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
        self.0.set_activity(None);
        self.0.worker_active.store(false, Ordering::SeqCst);
    }
}

/// ToolGateway wrapper injecting the same boundary checks.
pub struct ControlledGateway {
    inner: McpGateway,
    ctl: Arc<RunControl>,
}

impl trollm_mcp::ToolGateway for ControlledGateway {
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
}

fn load_run_plan(base: &Path, flow_sha: &str) -> Result<RunPlan> {
    let dir = base.join("bundles").join(flow_sha);
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
        .map(|server| authentication.bind(server, token_provider.as_ref()))
        .collect::<Result<Vec<_>>>()?;
    let policies = PolicySet::from_yaml_value(&serde_yaml::to_value(&run_plan.plan.mcp.policies)?)?;
    let sink = proxy_sink(
        run_dir.path().join("proxy.events.jsonl"),
        run_dir.run_id().to_string(),
    )?;
    McpGateway::new(
        servers,
        policies,
        base.join("cache").join("mcp"),
        run_dir.artifacts_dir(),
        Some(sink),
    )?
    .with_argument_constraints(run_plan.argument_constraints.clone())
}

// ---------------------------------------------------------------------------
// Model wiring (feature-gated; §9: weights are pinned by the app, never the
// plan — the plan contributes only slot names and ctx sizes).

/// Verify `models/<slot>.gguf` against its `models/<slot>.gguf.sha256`
/// manifest (§8). sha256sum-style lines ("<hex>  <name>") and bare hex are
/// both accepted.
#[cfg(any(feature = "llama", test))]
fn verify_model_weights(base: &Path, slot: &str, ctl: &RunControl) -> Result<PathBuf> {
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
    verify_weights(&path, &expected, slot, ctl)?;
    Ok(path)
}

#[cfg(any(feature = "llama", test))]
fn verify_weights(path: &Path, expected: &str, slot: &str, ctl: &RunControl) -> Result<()> {
    let mut file = fs::File::open(path)
        .with_context(|| format!("no weights for model slot '{slot}' ({})", path.display()))?;
    use sha2::Digest;
    let mut h = sha2::Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        ctl.before_boundary()?;
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
    fn generate(&mut self, req: GenerateRequest) -> Result<GenerateResponse> {
        let _activity = ActivityGuard(self.ctl.clone());
        self.ctl.before_boundary()?;
        if self.loaded.is_none() {
            self.ctl.set_activity(Some(RunActivityApi::VerifyingModel {
                slot: self.slot.clone(),
            }));
            let path = match &self.weights {
                Some((path, expected)) => {
                    verify_weights(path, expected, &self.slot, &self.ctl)?;
                    path.clone()
                }
                None => verify_model_weights(&self.base, &self.slot, &self.ctl)?,
            };
            self.ctl.before_boundary()?;
            self.ctl.set_activity(Some(RunActivityApi::LoadingModel {
                slot: self.slot.clone(),
            }));
            self.loaded = Some((self.loader)(&path)?);
        }
        self.ctl.before_boundary()?;
        self.ctl.set_activity(Some(RunActivityApi::Generating {
            slot: self.slot.clone(),
        }));
        self.loaded.as_mut().unwrap().generate(req)
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
    base: &Path,
    configuration: Option<&RunConfiguration>,
    ctl: Arc<RunControl>,
) -> Result<SlotMap<ControlledModel>> {
    let mut decls = Vec::new();
    slot_decls(plan, &mut decls);
    let mut map = SlotMap::new();
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
        map.insert(
            name.clone(),
            ControlledModel {
                slot: name,
                inner: Box::new(lazy),
                ctl: ctl.clone(),
            },
        );
    }
    Ok(map)
}

#[cfg(all(feature = "stub", not(feature = "llama")))]
fn build_slots(
    plan: &ExecutionPlan,
    _base: &Path,
    _configuration: Option<&RunConfiguration>,
    ctl: Arc<RunControl>,
) -> Result<SlotMap<ControlledModel>> {
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
    Ok(map)
}

#[cfg(not(any(feature = "llama", feature = "stub")))]
fn build_slots(
    _plan: &ExecutionPlan,
    _base: &Path,
    _configuration: Option<&RunConfiguration>,
    _ctl: Arc<RunControl>,
) -> Result<SlotMap<ControlledModel>> {
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
#[cfg_attr(
    not(any(feature = "llama", feature = "stub")),
    allow(unused_variables, unreachable_code)
)]
pub fn spawn_run(
    base: PathBuf,
    flow_sha: String,
    user_input: String,
    token_provider: Option<SharedTokenProvider>,
    resume_run_id: Option<String>,
    configuration: Option<RunConfiguration>,
) -> Result<SpawnedRun> {
    // Fail synchronously when this build carries no model runtime at all.
    #[cfg(not(any(feature = "llama", feature = "stub")))]
    bail!(TrollmError::ModelUnavailable(
        "this build has no model runtime: built without the 'llama' feature (and without the 'stub' test feature)".into()
    ));

    let mut run_plan = load_run_plan(&base, &flow_sha)?;
    let user_input = match crate::bundle::installed_flow_setup(&base, &flow_sha)?
        .and_then(|setup| setup.input)
    {
        Some(input) => {
            let resolved = input.resolve(&user_input)?;
            run_plan.argument_constraints = resolved.argument_constraints;
            resolved.canonical
        }
        None => user_input,
    };
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

    // Resume always uses recorded effective settings; UI/default changes cannot
    // silently select different weights or endpoints for an existing run.
    let mut configuration = if resume_run_id.is_some() {
        RunConfiguration::load(run_dir.path())?
    } else {
        configuration
    };
    if let Some(configuration) = &mut configuration {
        configuration.validate_and_resolve(&run_plan.plan)?;
        configuration.apply_to_plan(&mut run_plan.plan);
    }

    let status = Arc::new(Mutex::new(RunPhase::Running));
    let control = Arc::new(RunControl {
        cancel: AtomicBool::new(false),
        worker_active: AtomicBool::new(true),
        volatile_worker_failure: AtomicBool::new(false),
        activity: Mutex::new(Some(RunActivityApi::Starting)),
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

    // user_input persistence for cold resume (wrapper sidecar — RunState
    // does not carry it; documented in OPEN QUESTIONS).
    if resume_run_id.is_none() {
        run_dir.create_dirs()?;
        fs::write(run_dir.path().join("user_input.txt"), &user_input)?;
        if let Some(configuration) = &configuration {
            configuration.save(run_dir.path())?;
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
        })
        .context("spawning run worker")?;

    Ok(SpawnedRun { handle, run_id })
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
        inner: gateway,
        ctl: ctl.clone(),
    };
    let slots = build_slots(&run_plan.plan, &base, configuration.as_ref(), ctl.clone())?;
    let mut exec = Executor::new(
        run_plan.plan,
        run_plan.registry,
        user_input,
        slots,
        gateway,
        run_dir,
    )?;
    exec.set_control(ctl.clone());
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
        ctl.set_activity(None);
        let terminal = !matches!(phase, RunPhase::Running | RunPhase::Paused { .. });
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
            *ctl.status.lock().unwrap() = RunPhase::Failed {
                reason: "cancelled".to_string(),
            };
            return Ok(());
        }
        // Plan-driven pause: park until resume/cancel.
        match rx.recv() {
            Ok(RunCommand::Resume) => {
                result = exec.resume();
            }
            Ok(RunCommand::Cancel) => {
                exec.try_cancel()?;
                *ctl.status.lock().unwrap() = RunPhase::Failed {
                    reason: "cancelled".to_string(),
                };
                return Ok(());
            }
            Err(_) => return Ok(()), // runtime dropped; snapshot is persisted
        }
    }
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
        .and_then(|frame| frame.node_outputs.get(&last_done_node))
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
    fn tool_free_gateway_ignores_optional_mcp_trust_configuration() {
        let base = TrustTestDir::new("tool-free");
        fs::write(base.0.join("trust/mcp-roots.pem"), b"invalid").unwrap();
        let source = include_str!("../../../flows/examples/minimal-summary.flow.yaml");
        let run_dir = RunDir::under(&base.0);
        fs::create_dir_all(run_dir.path()).unwrap();
        let run_plan = |plan_source: &str| RunPlan {
            configuration: None,
            argument_constraints: Default::default(),
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
            cancel: AtomicBool::new(false),
            worker_active: AtomicBool::new(true),
            volatile_worker_failure: AtomicBool::new(false),
            activity: Mutex::new(Some(RunActivityApi::Starting)),
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
                verify_model_weights(&base, "coder", &handle().0.control).unwrap(),
                weights
            );
        }
        fs::write(&weights, b"abd").unwrap();
        assert!(verify_model_weights(&base, "coder", &handle().0.control)
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
        assert!(lazy
            .generate(request())
            .unwrap_err()
            .to_string()
            .contains("native allocation failed"));
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
        assert_eq!(status.activity, Some(RunActivityApi::Starting));
        *handle.status.lock().unwrap() = RunPhase::Paused {
            reason: "hold".into(),
        };
        let status = rt.poll_status(ID.into()).unwrap();
        assert!(status.worker_active);
        assert_eq!(status.activity, None);
        handle.resume();
        assert!(matches!(*handle.status.lock().unwrap(), RunPhase::Running));
        *handle.status.lock().unwrap() = RunPhase::Succeeded;
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
