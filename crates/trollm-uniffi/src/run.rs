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
//! - **user-initiated cancel of a *running* run**: the flag is checked at
//!   every model/tool boundary; the wrapper then returns a marker error so
//!   the run terminates promptly. The event log records this truthfully as a
//!   `model-error`/`mcp-error` failure (the marker string is greppable);
//!   `poll_status` translates it to `Failed{reason:"cancelled"}`.
//!   Cancellation of a *paused* run uses `Executor::cancel()` and logs
//!   `run_ended{outcome:"cancelled"}` exactly as core intends.
//!
//! Proxy events (§6): `McpGateway`'s sink cannot append to the run's main
//! `events.jsonl` without a core change — `Executor` owns the log and its
//! `seq` counter, so a second writer would break §7's monotonic `seq`. The
//! sink therefore writes `runs/<uuid>/proxy.events.jsonl` (same envelope
//! shape, native proxy event kinds, sidecar-local seq). Reported as an open
//! question; `tail_events` reads the authoritative main log only.

use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex};
use trollm_core::exec::SlotMap;
use trollm_core::{EventType, ExecutionPlan, Executor, RunDir, RunStatus, TaskRegistry};
use trollm_mcp::{EventSink, McpGateway, PolicySet, ProxyEvent, RootCertificates, ServerConfig};
use trollm_models::{GenerateRequest, GenerateResponse, ModelSlot};

use crate::authentication::RunAuthentication;
use crate::bundle::{BundleManifest, MANIFEST_NAME};
use crate::configuration::RunConfiguration;
use crate::SharedTokenProvider;

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

/// ModelSlot wrapper injecting pause/cancel checks around the real slot.
pub struct ControlledModel {
    inner: Box<dyn ModelSlot>,
    ctl: Arc<RunControl>,
}

impl ModelSlot for ControlledModel {
    fn generate(&mut self, req: GenerateRequest) -> Result<GenerateResponse> {
        self.ctl.before_boundary()?;
        let resp = self.inner.generate(req)?;
        self.ctl.before_boundary()?;
        Ok(resp)
    }
}

/// ToolGateway wrapper injecting the same boundary checks.
pub struct ControlledGateway {
    inner: McpGateway,
    ctl: Arc<RunControl>,
}

impl trollm_mcp::ToolGateway for ControlledGateway {
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
    let manifest: BundleManifest = serde_json::from_slice(
        &fs::read(dir.join(MANIFEST_NAME))
            .with_context(|| format!("no installed flow bundle for sha256 {flow_sha}"))?,
    )?;
    let (plan, registry) = crate::bundle::validate_materialized_bundle(&dir, &manifest)?;
    if plan.sha256 != flow_sha {
        bail!("installed entry flow sha256 mismatch (corrupt bundles dir)");
    }
    Ok(RunPlan {
        plan,
        registry,
        configuration: None,
        argument_constraints: Default::default(),
    })
}

/// Append a proxy event to the run's sidecar log (see module docs).
fn proxy_sink(events_path: PathBuf, run_id: String) -> EventSink {
    let seq = AtomicU64::new(0);
    let file = Mutex::new(
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&events_path),
    );
    Box::new(move |event: ProxyEvent| {
        let mut guard = file.lock().unwrap();
        let Ok(file) = guard.as_mut() else { return };
        let seq = seq.fetch_add(1, Ordering::SeqCst) + 1;
        let mut obj = json!({
            "ts": trollm_core::events::now_ms(),
            "runId": run_id,
            "seq": seq,
            "type": event.kind(),
        });
        if let (Value::Object(base), Ok(Value::Object(extra))) =
            (&mut obj, serde_json::to_value(&event))
        {
            for (k, v) in extra {
                if k != "type" {
                    base.insert(k.clone(), v.clone());
                }
            }
        }
        use std::io::Write;
        let _ = writeln!(file, "{obj}");
        let _ = file.flush();
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
    );
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
fn verify_model_weights(base: &Path, slot: &str) -> Result<PathBuf> {
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
    verify_weights(&path, &expected, slot)?;
    Ok(path)
}

#[cfg(any(feature = "llama", test))]
fn verify_weights(path: &Path, expected: &str, slot: &str) -> Result<()> {
    let mut file = fs::File::open(path)
        .with_context(|| format!("no weights for model slot '{slot}' ({})", path.display()))?;
    use sha2::Digest;
    let mut h = sha2::Sha256::new();
    std::io::copy(&mut file, &mut h)?;
    let actual = format!("{:x}", h.finalize());
    if !actual.eq_ignore_ascii_case(expected) {
        bail!(
            "model weights sha256 mismatch for slot '{slot}': manifest {expected}, actual {actual}"
        );
    }
    Ok(())
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
        let (path, options) = match configuration {
            Some(configuration) => {
                let model = &configuration.models[&name];
                let path = PathBuf::from(&model.path);
                verify_weights(&path, &model.sha256, &name)?;
                (
                    path,
                    trollm_models::llama::LlamaCppOptions {
                        temperature: model.temperature as f32,
                        top_p: model.top_p as f32,
                        ..Default::default()
                    },
                )
            }
            None => (verify_model_weights(base, &name)?, Default::default()),
        };
        let slot = trollm_models::llama::LlamaCppSlot::with_options(&path, ctx, options)?;
        map.insert(
            name,
            ControlledModel {
                inner: Box::new(slot),
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
            name,
            ControlledModel {
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
    bail!(
        "this build has no model runtime: built without the 'llama' feature \
         (and without the 'stub' test feature)"
    );

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
                bail!("no resumable snapshot for run {id}");
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
    }

    let thread_control = control.clone();
    let thread_status = handle.status.clone();
    let resume_existing = resume_run_id.is_some();
    run_plan.configuration = configuration;
    let failure_dir = run_dir.path().to_path_buf();
    std::thread::Builder::new()
        .name(format!("trollm-run-{run_id}"))
        .spawn(move || {
            let result = drive(
                run_plan,
                base,
                run_dir,
                user_input,
                token_provider,
                (thread_control, rx),
                resume_existing,
            );
            if let Err(e) = result {
                let mut reason = format!("run worker error: {e:#}");
                let bytes = serde_json::to_vec(&json!({"reason": reason}))
                    .expect("string failure record serializes");
                if let Err(persist_error) =
                    trollm_core::artifacts::write_atomic(&failure_dir, WORKER_FAILURE_FILE, &bytes)
                {
                    reason.push_str(&format!(" (could not persist failure: {persist_error})"));
                }
                *thread_status.lock().unwrap() = RunPhase::Failed { reason };
            }
        })
        .context("spawning run worker")?;

    Ok(SpawnedRun { handle, run_id })
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
        let phase = match &result {
            Ok(RunStatus::Running) => RunPhase::Running,
            Ok(RunStatus::Paused { reason }) => RunPhase::Paused {
                reason: reason.clone(),
            },
            Ok(RunStatus::Succeeded) => RunPhase::Succeeded,
            Ok(RunStatus::Failed { reason }) => RunPhase::Failed {
                reason: reason.clone(),
            },
            Err(e) => RunPhase::Failed {
                reason: format!("{e:#}"),
            },
        };
        // Cancel of a running run surfaces as a marker-error failure in the
        // log (see module docs); report it to the UI as cancelled.
        let phase = match phase {
            RunPhase::Failed { .. } if ctl.is_cancelled() => RunPhase::Failed {
                reason: "cancelled".to_string(),
            },
            other => other,
        };
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
            exec.cancel();
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
                exec.cancel();
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
            reason: "failed (persisted)".to_string(),
        },
    }))
}

/// The plan sha256 a persisted run belongs to, for cold resume.
pub fn plan_sha_of_run(base: &Path, run_id: &str) -> Result<String> {
    let snapshot = base.join("runs").join(run_id).join("state.snapshot.json");
    let state: trollm_core::RunState =
        serde_json::from_slice(&fs::read(&snapshot)?).context("parsing state.snapshot.json")?;
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
    let mut last = from_seq;
    if !path.exists() {
        return Ok((out, last));
    }
    for line in std::io::BufRead::lines(std::io::BufReader::new(fs::File::open(&path)?)) {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let Some(seq) = v.get("seq").and_then(Value::as_u64) else {
            continue;
        };
        last = last.max(seq);
        if seq > from_seq {
            let ty = v
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            out.push((seq, ty, line));
        }
    }
    Ok((out, last))
}

/// Final report heuristic (see OPEN QUESTIONS in the implementation report):
/// §9 has no result accessor, so on `Succeeded` the report is recovered from
/// the run's own records — the last root-frame node to transition
/// `onVerified` into an `end` node (from the event log), whose bound prose
/// outputs are read back out of `state.snapshot.json`. The wrapper passes
/// prose through verbatim; it never parses it.
pub fn final_report(base: &Path, run_id: &str) -> Option<String> {
    let dir = base.join("runs").join(run_id);
    let events = trollm_core::EventLog::read_all(&dir.join("events.jsonl")).ok()?;
    let last_done_node = events.iter().rev().find_map(|e| {
        if e.get("type")?.as_str()? == EventType::Transition.as_str()
            && e.get("key")?.as_str()? == "onVerified"
        {
            e.get("from")?.as_str().map(str::to_string)
        } else {
            None
        }
    })?;
    let state: trollm_core::RunState =
        serde_json::from_slice(&fs::read(dir.join("state.snapshot.json")).ok()?).ok()?;
    let frame = state.stack.first()?;
    let outputs = frame.node_outputs.get(&last_done_node)?;
    let mut parts: Vec<&str> = outputs.values().filter_map(Value::as_str).collect();
    parts.sort_unstable();
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("\n"))
    }
}

/// Registry of active runs in this process.
#[derive(Default)]
pub struct RunRegistry {
    runs: Mutex<HashMap<String, Arc<RunHandle>>>,
}

impl RunRegistry {
    pub fn insert(&self, run_id: String, handle: Arc<RunHandle>) {
        self.runs.lock().unwrap().insert(run_id, handle);
    }

    pub fn get(&self, run_id: &str) -> Option<Arc<RunHandle>> {
        self.runs.lock().unwrap().get(run_id).cloned()
    }

    pub fn has_active(&self) -> bool {
        self.runs.lock().unwrap().values().any(|handle| {
            matches!(
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

    fn handle() -> (Arc<RunHandle>, Receiver<RunCommand>) {
        let status = Arc::new(Mutex::new(RunPhase::Running));
        let control = Arc::new(RunControl {
            cancel: AtomicBool::new(false),
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
            assert_eq!(verify_model_weights(&base, "coder").unwrap(), weights);
        }
        fs::write(&weights, b"abd").unwrap();
        assert!(verify_model_weights(&base, "coder")
            .unwrap_err()
            .to_string()
            .contains("mismatch"));
        fs::remove_dir_all(base).unwrap();
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
