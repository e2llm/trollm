//! trollm-uniffi: the thin UniFFI wrapper over trollm-core /
//! trollm-models / trollm-mcp (docs/ARCHITECTURE-ANDROID.md §9).
//!
//! Marshalling only — all orchestration logic lives in core. Exposes exactly
//! the §9 surface, as methods on the `TrollmRuntime` object (the
//! constructor is the wiring entrypoint):
//!
//! `list_flows`, `install_flow_bundle`, `start_run`, `poll_status`,
//! `pause`, `resume`, `cancel`, `tail_events`.
//!
//! UniFFI mechanics (may-decide, §13): **proc-macro mode** (`uniffi = "0.28"`,
//! `#[uniffi::export]`, no UDL file, no build.rs). Kotlin bindings are
//! generated with `uniffi-bindgen generate --library` (see
//! apps/android/README.md); the crate builds as `cdylib` + `rlib`.

mod authentication;
pub mod bundle;
pub mod configuration;
pub mod run;
pub mod setup;
pub mod tar;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use run::{RunPhase, RunRegistry};
use trollm_core::ExecutionPlan;

pub use authentication::ConnectionAuth;

uniffi::setup_scaffolding!();

/// Shared handle to the app-supplied token provider. UniFFI 0.28 callback
/// interfaces are passed across the FFI as `Box<dyn Trait>`; we wrap it in
/// `Arc` internally so per-run gateways can clone it.
pub(crate) type SharedTokenProvider = Arc<Box<dyn TokenProvider>>;

/// Errors surfaced to Kotlin. Flat messages; the underlying anyhow context
/// chain is preserved in the text.
#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum TrollmError {
    #[error("io: {0}")]
    Io(String),
    #[error("bundle rejected: {0}")]
    Bundle(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("run error: {0}")]
    Run(String),
    #[error("model runtime unavailable: {0}")]
    ModelUnavailable(String),
}

impl From<anyhow::Error> for TrollmError {
    fn from(e: anyhow::Error) -> Self {
        let msg = format!("{e:#}");
        let lower = msg.to_lowercase();
        if lower.contains("no model runtime") || lower.contains("weights") {
            Self::ModelUnavailable(msg)
        } else if lower.contains("no installed flow") || lower.contains("no resumable") {
            Self::NotFound(msg)
        } else if lower.contains("bundle") || lower.contains("manifest") || lower.contains("sha256")
        {
            Self::Bundle(msg)
        } else {
            Self::Run(msg)
        }
    }
}

/// Application-supplied bearer tokens for MCP servers (§3: "auth resolved by
/// the app, not the plan"). The Android side backs this with
/// Keystore/EncryptedSharedPreferences.
#[uniffi::export(callback_interface)]
pub trait TokenProvider: Send + Sync {
    /// Bearer token for the named MCP server, or null for none.
    fn bearer_token(&self, server_name: String) -> Option<String>;

    /// Select a non-secret credential identity once for a new run. The endpoint
    /// has been validated before this callback. Existing Rust manual providers
    /// retain their original behavior through this default implementation.
    fn connection_auth(&self, server_name: String, server_url: String) -> ConnectionAuth {
        let _ = server_url;
        if self.bearer_token(server_name).is_some() {
            ConnectionAuth::Bearer
        } else {
            ConnectionAuth::Anonymous
        }
    }

    /// Return a fresh token only for the exact saved grant and endpoint. Null
    /// means reconnect is required; it must never select another account.
    fn oauth_token(
        &self,
        server_name: String,
        server_url: String,
        grant_id: String,
    ) -> Option<String> {
        let _ = (server_name, server_url, grant_id);
        None
    }
}

/// One installed flow (§8 canonical pin `flows/<sha256>.flow.yaml`).
#[derive(Debug, Clone, uniffi::Record)]
pub struct FlowInfo {
    pub sha256: String,
    pub id: String,
    pub version: u32,
}

/// Result of `install_flow_bundle`: every file sha256-verified, plan passed
/// core validation.
#[derive(Debug, Clone, uniffi::Record)]
pub struct InstallResult {
    pub flow_sha256: String,
    pub flow_id: String,
    pub flow_version: u32,
    pub files_verified: u32,
    /// Install location of the materialized bundle (under `bundles/`).
    pub bundle_dir: String,
    /// Honest signature status — verification is deferred (tech-debt #1),
    /// so this currently always says so.
    pub signature_status: String,
}

/// Run status (§9 RunStatus marshalled; see crate::run docs for the
/// cooperative user-pause/cancel semantics).
#[derive(Debug, Clone, uniffi::Enum)]
pub enum RunPhaseApi {
    Running,
    Paused { reason: String },
    Succeeded,
    Failed { reason: String },
}

impl From<&RunPhase> for RunPhaseApi {
    fn from(p: &RunPhase) -> Self {
        match p {
            RunPhase::Running => Self::Running,
            RunPhase::Paused { reason } => Self::Paused {
                reason: reason.clone(),
            },
            RunPhase::Succeeded => Self::Succeeded,
            RunPhase::Failed { reason } => Self::Failed {
                reason: reason.clone(),
            },
        }
    }
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct RunStatusInfo {
    pub run_id: String,
    pub phase: RunPhaseApi,
    /// Highest seq observed in events.jsonl (cursor helper for the UI).
    pub last_seq: u64,
    /// On Succeeded: the final report, recovered from the run's own records
    /// (see `run::final_report`; verbatim prose, never parsed).
    pub final_report: Option<String>,
}

/// One event-log line (§7), verbatim — the UI renders from `event_type` and
/// the raw JSON only.
#[derive(Debug, Clone, uniffi::Record)]
pub struct EventLine {
    pub seq: u64,
    pub event_type: String,
    /// The complete JSONL line, unmodified.
    pub json: String,
}

/// The runtime entrypoint: wires the §8 storage layout under the app's
/// files dir, the model slots (llama feature) and the per-run MCP gateway.
#[derive(uniffi::Object)]
pub struct TrollmRuntime {
    /// `<app filesDir>/trollm` (§8 root).
    base: PathBuf,
    token_provider: Mutex<Option<SharedTokenProvider>>,
    runs: RunRegistry,
}

#[uniffi::export]
impl TrollmRuntime {
    /// `files_dir` is the app's files dir; the §8 tree lives under
    /// `<files_dir>/trollm/`.
    #[uniffi::constructor]
    pub fn new(
        files_dir: String,
        token_provider: Option<Box<dyn TokenProvider>>,
    ) -> Result<Arc<Self>, TrollmError> {
        let base = Path::new(&files_dir).join("trollm");
        for sub in ["flows", "tasks", "bundles", "models", "runs", "cache/mcp"] {
            std::fs::create_dir_all(base.join(sub)).map_err(|e| {
                TrollmError::Io(format!("creating {}: {e}", base.join(sub).display()))
            })?;
        }
        Ok(Arc::new(Self {
            base,
            token_provider: Mutex::new(token_provider.map(Arc::new)),
            runs: RunRegistry::default(),
        }))
    }

    /// §9: list installed flows (from the canonical §8 pins).
    pub fn list_flows(&self) -> Result<Vec<FlowInfo>, TrollmError> {
        let mut out = Vec::new();
        let flows_dir = self.base.join("flows");
        let entries = std::fs::read_dir(&flows_dir)
            .map_err(|e| TrollmError::Io(format!("reading {}: {e}", flows_dir.display())))?;
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            let Some(sha) = name.strip_suffix(".flow.yaml") else {
                continue;
            };
            let Ok(bytes) = std::fs::read(entry.path()) else {
                continue;
            };
            let Ok(text) = String::from_utf8(bytes) else {
                continue;
            };
            // Parse for id/version; corrupt pins are skipped, not fatal.
            if let Ok(plan) = ExecutionPlan::from_yaml_str(&text) {
                out.push(FlowInfo {
                    sha256: sha.to_string(),
                    id: plan.id,
                    version: plan.version,
                });
            }
        }
        out.sort_by(|a, b| a.id.cmp(&b.id).then(a.sha256.cmp(&b.sha256)));
        Ok(out)
    }

    /// §9: install a flow bundle (uncompressed ustar tar — see `tar.rs` for
    /// the format decision). Fails closed: every file's sha256 must match the
    /// manifest and the plan must pass core validation, or nothing is
    /// installed.
    pub fn install_flow_bundle(&self, bytes: Vec<u8>) -> Result<InstallResult, TrollmError> {
        let outcome = bundle::install_bundle(&self.base, &bytes).map_err(TrollmError::from)?;
        Ok(InstallResult {
            flow_sha256: outcome.flow_sha256,
            flow_id: outcome.flow_id,
            flow_version: outcome.flow_version,
            files_verified: outcome.files_verified,
            bundle_dir: outcome.bundle_dir.to_string_lossy().to_string(),
            signature_status: outcome.signature_status,
        })
    }

    /// Verified, pinned setup defaults for an installed flow. Legacy bundles
    /// without a descriptor return None. No credentials are part of this JSON.
    pub fn flow_setup(&self, flow_sha256: String) -> Result<Option<String>, TrollmError> {
        let setup =
            bundle::installed_flow_setup(&self.base, &flow_sha256).map_err(TrollmError::from)?;
        setup
            .map(|value| serde_json::to_string(&value))
            .transpose()
            .map_err(|e| TrollmError::Bundle(format!("serializing flow setup: {e}")))
    }

    /// UI admission hint: keep one native inference run active by default.
    /// Core callers may still explicitly use concurrent runs.
    pub fn has_active_run(&self) -> bool {
        self.runs.has_active()
    }

    /// §9: start a run of the flow pinned by `flow_sha256`. The run executes
    /// on a background thread; observe it via `poll_status` / `tail_events`.
    pub fn start_run(
        &self,
        flow_sha256: String,
        user_input: String,
    ) -> Result<String, TrollmError> {
        let spawned = run::spawn_run(
            self.base.clone(),
            flow_sha256,
            user_input,
            self.token_provider.lock().unwrap().clone(),
            None,
            None,
        )
        .map_err(TrollmError::from)?;
        let run_id = spawned.run_id.clone();
        self.runs.insert(run_id.clone(), spawned.handle);
        Ok(run_id)
    }

    /// Start with app-resolved model files and connection overrides. The
    /// effective settings are validated and persisted for cold resume.
    pub fn start_run_configured(
        &self,
        flow_sha256: String,
        user_input: String,
        configuration_json: String,
    ) -> Result<String, TrollmError> {
        let configuration = serde_json::from_str(&configuration_json)
            .map_err(|e| TrollmError::Run(format!("invalid run configuration: {e}")))?;
        let spawned = run::spawn_run(
            self.base.clone(),
            flow_sha256,
            user_input,
            self.token_provider.lock().unwrap().clone(),
            None,
            Some(configuration),
        )
        .map_err(TrollmError::from)?;
        let run_id = spawned.run_id.clone();
        self.runs.insert(run_id.clone(), spawned.handle);
        Ok(run_id)
    }

    /// §9: current status. Falls back to the persisted snapshot for runs not
    /// in this process (app restart), so the UI survives process death.
    pub fn poll_status(&self, run_id: String) -> Result<RunStatusInfo, TrollmError> {
        let phase = match self.runs.get(&run_id) {
            Some(handle) => handle.status.lock().unwrap().clone(),
            None => run::status_from_disk(&self.base, &run_id)
                .map_err(TrollmError::from)?
                .ok_or_else(|| TrollmError::NotFound(format!("no run '{run_id}'")))?,
        };
        let (_, last_seq) = run::tail_events(&self.base, &run_id, 0).map_err(TrollmError::from)?;
        let final_report = if matches!(phase, RunPhase::Succeeded) {
            run::final_report(&self.base, &run_id)
        } else {
            None
        };
        Ok(RunStatusInfo {
            run_id,
            phase: RunPhaseApi::from(&phase),
            last_seq,
            final_report,
        })
    }

    /// §9: pause. Plan-paused runs stay paused; running runs get a
    /// cooperative hold at the next model/tool boundary (see `run.rs` docs).
    pub fn pause(&self, run_id: String) -> Result<(), TrollmError> {
        let handle = self
            .runs
            .get(&run_id)
            .ok_or_else(|| TrollmError::NotFound(format!("no active run '{run_id}'")))?;
        handle.pause();
        Ok(())
    }

    /// §9: resume. Works for the cooperative hold, for plan-driven pauses,
    /// and for cold resume after process death (rebuilds the executor over
    /// the persisted snapshot — core's crash-recovery path, §4.4).
    pub fn resume(&self, run_id: String) -> Result<(), TrollmError> {
        if let Some(handle) = self.runs.get(&run_id) {
            handle.resume();
            return Ok(());
        }
        // Cold resume: reconstruct from the persisted state.
        let flow_sha = run::plan_sha_of_run(&self.base, &run_id).map_err(TrollmError::from)?;
        let user_input =
            std::fs::read_to_string(self.base.join("runs").join(&run_id).join("user_input.txt"))
                .unwrap_or_default();
        let spawned = run::spawn_run(
            self.base.clone(),
            flow_sha,
            user_input,
            self.token_provider.lock().unwrap().clone(),
            Some(run_id.clone()),
            None,
        )
        .map_err(TrollmError::from)?;
        self.runs.insert(run_id, spawned.handle);
        Ok(())
    }

    /// §9: cancel. Paused runs are cancelled through `Executor::cancel`
    /// (logged as `run_ended{outcome:"cancelled"}`); running runs terminate
    /// at the next model/tool boundary via the marker-error path documented
    /// in `run.rs`.
    pub fn cancel(&self, run_id: String) -> Result<(), TrollmError> {
        let handle = self
            .runs
            .get(&run_id)
            .ok_or_else(|| TrollmError::NotFound(format!("no active run '{run_id}'")))?;
        handle.cancel();
        Ok(())
    }

    /// §9: events with `seq > from_seq` (pass 0 for the whole log), from the
    /// run's authoritative `events.jsonl`.
    pub fn tail_events(
        &self,
        run_id: String,
        from_seq: u64,
    ) -> Result<Vec<EventLine>, TrollmError> {
        let (events, _) =
            run::tail_events(&self.base, &run_id, from_seq).map_err(TrollmError::from)?;
        if events.is_empty()
            && !self
                .base
                .join("runs")
                .join(&run_id)
                .join("events.jsonl")
                .exists()
            && !self
                .base
                .join("runs")
                .join(&run_id)
                .join(run::WORKER_FAILURE_FILE)
                .exists()
        {
            return Err(TrollmError::NotFound(format!("no run '{run_id}'")));
        }
        Ok(events
            .into_iter()
            .map(|(seq, event_type, json)| EventLine {
                seq,
                event_type,
                json,
            })
            .collect())
    }
}
