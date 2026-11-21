//! trollm-uniffi: the thin UniFFI wrapper over trollm-core /
//! trollm-models / trollm-mcp (docs/ARCHITECTURE-ANDROID.md §9).
//!
//! Marshalling only — all orchestration logic lives in core. Exposes exactly
//! the §9 surface, as methods on the `TrollmRuntime` object (the
//! constructor is the wiring entrypoint):
//!
//! `list_flows`, `install_flow_bundle`, `start_run`, `poll_status`,
//! `pause`, `resume`, `cancel`, `tail_events`, `run_summary`.
//!
//! UniFFI mechanics (may-decide, §13): **proc-macro mode** (`uniffi = "0.28"`,
//! `#[uniffi::export]`, no UDL file, no build.rs). Kotlin bindings are
//! generated with `uniffi-bindgen generate --library` (see
//! apps/android/README.md); the crate builds as `cdylib` + `rlib`.

mod authentication;
pub mod bundle;
pub mod collection;
pub mod collection_authority;
mod collection_clock;
mod collection_installation;
pub mod collection_members;
pub mod collection_provider;
pub mod configuration;
mod history;
mod observation;
pub mod run;
mod selected_start;
pub mod setup;
mod source_report;
pub mod tar;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use run::{RunPhase, RunRegistry};
use trollm_core::ExecutionPlan;

pub use authentication::ConnectionAuth;
pub use history::RunSummaryInfo;

/// Canonical credential identity for an MCP endpoint, shared by host adapters.
pub fn canonical_mcp_endpoint(value: &str) -> anyhow::Result<String> {
    authentication::endpoint(value)
}

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
    #[error("selected start refused: {code}")]
    SelectedStart {
        code: String,
        native_run_id: Option<String>,
    },
}

impl TrollmError {
    fn typed_cause(error: &anyhow::Error) -> Option<Self> {
        let cause = error
            .chain()
            .find_map(|cause| cause.downcast_ref::<Self>())?;
        // A typed cause already contributes its public prefix to anyhow's
        // display. Strip that prefix before applying the variant again.
        let rendered = format!("{error:#}");
        let inner = match cause {
            Self::Io(value)
            | Self::Bundle(value)
            | Self::NotFound(value)
            | Self::Run(value)
            | Self::ModelUnavailable(value) => value,
            Self::SelectedStart { code, .. } => code,
        };
        let message = rendered
            .strip_suffix(&cause.to_string())
            .map(|context| format!("{context}{inner}"))
            .unwrap_or(rendered);
        Some(match cause {
            Self::Io(_) => Self::Io(message),
            Self::Bundle(_) => Self::Bundle(message),
            Self::NotFound(_) => Self::NotFound(message),
            Self::Run(_) => Self::Run(message),
            Self::ModelUnavailable(_) => Self::ModelUnavailable(message),
            Self::SelectedStart {
                code,
                native_run_id,
            } => Self::SelectedStart {
                code: code.clone(),
                native_run_id: native_run_id.clone(),
            },
        })
    }

    /// Preserve the full anyhow context while choosing a category at the API
    /// boundary. Error text may contain user-controlled paths and is not a type.
    fn run(error: anyhow::Error) -> Self {
        if let Some(classified) = Self::typed_cause(&error) {
            return classified;
        }
        let message = format!("{error:#}");
        if error.root_cause().is::<std::io::Error>() {
            Self::Io(message)
        } else {
            Self::Run(message)
        }
    }

    fn bundle(error: anyhow::Error) -> Self {
        Self::typed_cause(&error).unwrap_or_else(|| Self::Bundle(format!("{error:#}")))
    }

    fn selected_start(error: anyhow::Error) -> Self {
        // anyhow's typed context is available through its own downcast, not
        // necessarily through the std::error::Error source iterator.
        let failure = error.downcast_ref::<selected_start::Failure>();
        Self::SelectedStart {
            code: failure
                .map_or("SELECTED_START_UNAVAILABLE", |cause| cause.code)
                .into(),
            native_run_id: failure.and_then(|cause| cause.native_run_id.clone()),
        }
    }
}

#[cfg(test)]
mod error_classification_tests {
    use super::TrollmError;

    #[test]
    fn ordinary_run_message_cannot_change_its_category() {
        let error =
            anyhow::anyhow!("ordinary execution failure at /bundle/weights.txt/manifest-sha256");
        assert!(matches!(TrollmError::run(error), TrollmError::Run(_)));
    }

    #[test]
    fn typed_unavailable_runtime_keeps_context_without_duplicate_prefix() {
        let error = anyhow::Error::new(TrollmError::ModelUnavailable("runtime absent".into()))
            .context("loading /bundle/manifest/weights.txt");
        let classified = TrollmError::run(error);
        assert!(matches!(&classified, TrollmError::ModelUnavailable(_)));
        assert_eq!(
            classified.to_string(),
            "model runtime unavailable: loading /bundle/manifest/weights.txt: runtime absent"
        );
    }
}

/// Application-supplied bearer tokens for MCP servers (§3: "auth resolved by
/// the app, not the plan"). The Android side backs this with
/// Keystore/EncryptedSharedPreferences.
#[uniffi::export(callback_interface)]
pub trait TokenProvider: Send + Sync {
    /// Application-owned authenticated publication read, never a model tool or
    /// MCP credential lookup. False includes unavailable/unimplemented clients.
    /// The Android implementation pins the original source grant and compares
    /// the current server descriptor with the retained installation descriptor.
    fn source_authority_current(&self, association_json: String) -> bool {
        let _ = association_json;
        false
    }

    /// Atomically return the selected manual token only when its stored endpoint
    /// matches server_url. Legacy name-only tokens must require explicit re-save.
    fn bearer_token(&self, server_name: String, server_url: String) -> Option<String>;

    /// Select a non-secret credential identity once for a new run. The endpoint
    /// has been validated before this callback. Secret retrieval must independently
    /// recheck the endpoint, since connection settings can change after selection.
    fn connection_auth(&self, server_name: String, server_url: String) -> ConnectionAuth {
        if self.bearer_token(server_name, server_url).is_some() {
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
    pub installation_sha256: String,
    pub archive_sha256: String,
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

/// Ephemeral progress of the worker in this process; never a durable event.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum RunActivityApi {
    Starting,
    VerifyingModel { slot: String },
    LoadingModel { slot: String },
    Generating { slot: String },
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct RunStatusInfo {
    pub run_id: String,
    pub phase: RunPhaseApi,
    /// Actual native worker lifetime, including terminal publication before
    /// teardown. Terminal phase alone is not an inactive acknowledgement.
    pub worker_active: bool,
    pub activity: Option<RunActivityApi>,
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

/// Bounded disposable observation, separate from strict disk APIs. A cursor
/// predating retained history is advanced with an explicit omission signal.
#[derive(Debug, Clone, uniffi::Record)]
pub struct RunObservation {
    pub status: RunStatusInfo,
    pub events: Vec<EventLine>,
    pub next_seq: u64,
    pub older_events_omitted: bool,
    pub has_more: bool,
}

/// The runtime entrypoint: wires the §8 storage layout under the app's
/// files dir, the model slots (llama feature) and the per-run MCP gateway.
#[derive(uniffi::Object)]
pub struct TrollmRuntime {
    /// `<app filesDir>/trollm` (§8 root).
    base: PathBuf,
    token_provider: Mutex<Option<SharedTokenProvider>>,
    runs: RunRegistry,
    /// Serializes explicit resume reconstruction and registry insertion only;
    /// workers execute independently after their handles have been registered.
    resume_lock: Mutex<()>,
    observations: Mutex<observation::ColdCache>,
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
            resume_lock: Mutex::new(()),
            observations: Mutex::new(observation::ColdCache::default()),
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
        let outcome = bundle::install_bundle(&self.base, &bytes).map_err(TrollmError::bundle)?;
        Ok(InstallResult {
            installation_sha256: outcome.installation_sha256,
            archive_sha256: outcome.archive_sha256,
            flow_sha256: outcome.flow_sha256,
            flow_id: outcome.flow_id,
            flow_version: outcome.flow_version,
            files_verified: outcome.files_verified,
            bundle_dir: outcome.bundle_dir.to_string_lossy().to_string(),
            signature_status: outcome.signature_status,
        })
    }

    /// Import a dependency-complete owned-source revision through the same
    /// verified installer. This does not grant model or MCP authority.
    pub fn install_source_bundle(&self, bytes: Vec<u8>) -> Result<InstallResult, TrollmError> {
        let outcome =
            bundle::install_source_bundle(&self.base, &bytes).map_err(TrollmError::bundle)?;
        Ok(InstallResult {
            installation_sha256: outcome.installation_sha256,
            archive_sha256: outcome.archive_sha256,
            flow_sha256: outcome.flow_sha256,
            flow_id: outcome.flow_id,
            flow_version: outcome.flow_version,
            files_verified: outcome.files_verified,
            bundle_dir: outcome.bundle_dir.to_string_lossy().to_string(),
            signature_status: outcome.signature_status,
        })
    }

    pub fn source_bundle_setup(
        &self,
        installation_sha256: String,
    ) -> Result<Option<String>, TrollmError> {
        bundle::revision_setup(&self.base, &installation_sha256)
            .map_err(TrollmError::bundle)?
            .map(|setup| serde_json::to_string(&setup))
            .transpose()
            .map_err(|e| TrollmError::Bundle(format!("serializing source setup: {e}")))
    }

    /// Validate original structured bytes against this exact installed setup and
    /// the APK's independent normalized serialization before creating a run.
    pub fn admit_source_input(
        &self,
        installation_sha256: String,
        raw_input: String,
        apk_normalized_input: String,
    ) -> Result<String, TrollmError> {
        let setup = bundle::revision_setup(&self.base, &installation_sha256)
            .map_err(TrollmError::bundle)?
            .and_then(|setup| setup.input)
            .ok_or_else(|| TrollmError::Bundle("structured source input unavailable".into()))?;
        let admitted = setup::admit_source_input(&setup, &raw_input, &apk_normalized_input)
            .map_err(TrollmError::bundle)?;
        serde_json::to_string(&admitted).map_err(|error| TrollmError::bundle(error.into()))
    }

    pub fn start_source_run(
        &self,
        flow_sha256: String,
        installation_sha256: String,
        user_input: String,
        association_json: String,
    ) -> Result<String, TrollmError> {
        let association = serde_json::from_str(&association_json)
            .map_err(|e| TrollmError::Bundle(format!("invalid source association: {e}")))?;
        let spawned = run::spawn_source_run(
            self.base.clone(),
            flow_sha256,
            user_input,
            self.token_provider.lock().unwrap().clone(),
            None,
            None,
            Some((installation_sha256, association)),
        )
        .map_err(TrollmError::run)?;
        let run_id = spawned.run_id.clone();
        self.runs.insert(run_id.clone(), spawned.handle);
        Ok(run_id)
    }

    pub fn start_source_run_configured(
        &self,
        flow_sha256: String,
        installation_sha256: String,
        user_input: String,
        configuration_json: String,
        association_json: String,
    ) -> Result<String, TrollmError> {
        let association = serde_json::from_str(&association_json)
            .map_err(|e| TrollmError::Bundle(format!("invalid source association: {e}")))?;
        let configuration = serde_json::from_str(&configuration_json)
            .map_err(|e| TrollmError::Run(format!("invalid run configuration: {e}")))?;
        let spawned = run::spawn_source_run(
            self.base.clone(),
            flow_sha256,
            user_input,
            self.token_provider.lock().unwrap().clone(),
            None,
            Some(configuration),
            Some((installation_sha256, association)),
        )
        .map_err(TrollmError::run)?;
        let run_id = spawned.run_id.clone();
        self.runs.insert(run_id.clone(), spawned.handle);
        Ok(run_id)
    }

    /// Explicit app-private installation entry. The caller retains the returned
    /// genuine run ID and uses poll_status.worker_active for teardown, not an
    /// outcome ACK or callback return. No public source flag selects this path.
    #[allow(clippy::too_many_arguments)]
    pub fn start_selected_source_run_configured(
        &self,
        flow_sha256: String,
        installation_sha256: String,
        user_input: String,
        configuration_json: String,
        association_json: String,
        private_installation_key: String,
        invocation_key: String,
    ) -> Result<String, TrollmError> {
        let _start = self.resume_lock.lock().unwrap();
        let installation = collection_installation::load(
            &self.base,
            &private_installation_key,
            &installation_sha256,
            &invocation_key,
        )
        .map_err(|_| TrollmError::SelectedStart {
            code: "SELECTED_INSTALLATION_LOAD".into(),
            native_run_id: None,
        })?;
        let association =
            serde_json::from_str(&association_json).map_err(|_| TrollmError::SelectedStart {
                code: "SELECTED_SOURCE_JSON".into(),
                native_run_id: None,
            })?;
        let configuration =
            serde_json::from_str(&configuration_json).map_err(|_| TrollmError::SelectedStart {
                code: "SELECTED_CONFIGURATION_JSON".into(),
                native_run_id: None,
            })?;
        let spawned = run::spawn_installed_source_run(
            self.base.clone(),
            flow_sha256,
            user_input,
            self.token_provider.lock().unwrap().clone(),
            None,
            Some(configuration),
            Some((installation_sha256, association)),
            Some(installation),
        )
        .map_err(TrollmError::selected_start)?;
        let run_id = spawned.run_id.clone();
        self.runs.insert(run_id.clone(), spawned.handle);
        Ok(run_id)
    }

    /// Same-process selected-worker readback. Missing/evicted handles refuse;
    /// absence is never translated into an inactive-worker acknowledgement.
    pub fn selected_run_binding(&self, run_id: String) -> Result<String, TrollmError> {
        history::validate_run_id(&run_id)?;
        let handle = self
            .runs
            .get(&run_id)
            .ok_or_else(|| TrollmError::Run("selected live worker handle unavailable".into()))?;
        let (installed_key, installed_invocation) = handle
            .control
            .selected_invocation_binding()
            .map_err(TrollmError::run)?;
        let directory = self.base.join("runs").join(&run_id);
        let claim = setup::read_input_evidence(&directory, "selected-invocation.json", 65536)
            .map_err(TrollmError::run)?;
        let private =
            setup::read_input_evidence(&directory, "selected-private-installation.json", 4096)
                .map_err(TrollmError::run)?;
        let claim_value: serde_json::Value = serde_json::from_slice(&claim)
            .map_err(|_| TrollmError::Run("invalid native selected claim".into()))?;
        let private_value: serde_json::Value = serde_json::from_slice(&private)
            .map_err(|_| TrollmError::Run("invalid native private binding".into()))?;
        if claim_value["schema"] != "trollm.selected-run-claim.v1"
            || private_value["schema"] != "trollm.selected-private-installation-binding.v1"
            || private_value["privateInstallationSha256"] != installed_key
            || private_value["invocationKey"] != installed_invocation
            || claim_value["nativeRunId"] != run_id
            || private_value["nativeRunId"] != run_id
            || claim_value["identity"]["invocationKey"] != private_value["invocationKey"]
        {
            return Err(TrollmError::Run("selected worker association drift".into()));
        }
        let source = self.source_run_association(run_id.clone())?;
        serde_json::to_string(&serde_json::json!({
            "schema":"trollm.selected-worker-binding.v1", "nativeRunId":run_id,
            "workerActive":handle.control.worker_active(),
            "privateBindingJson":String::from_utf8(private)
                .map_err(|_| TrollmError::Run("invalid private binding encoding".into()))?,
            "claimJson":String::from_utf8(claim)
                .map_err(|_| TrollmError::Run("invalid claim encoding".into()))?,
            "sourceAssociationJson":source,
        }))
        .map_err(|e| TrollmError::Run(format!("selected binding serialization: {e}")))
    }

    pub fn is_source_run(&self, run_id: String) -> Result<bool, TrollmError> {
        history::validate_run_id(&run_id)?;
        let dir = self.base.join("runs").join(run_id);
        Ok(dir.join("installation.json").exists() || dir.join("source-association.json").exists())
    }

    /// Passive terminal observation. This never invokes models, tools or resume.
    pub fn prepare_source_report(
        &self,
        run_id: String,
        device_id: String,
    ) -> Result<String, TrollmError> {
        let _guard = self.resume_lock.lock().unwrap();
        let status = self.poll_status(run_id.clone())?;
        // Passive interrupted evidence is reportable without changing its phase.
        // The same lock excludes explicit reconstruction while custody is read.
        if status.worker_active {
            return Err(TrollmError::Run(
                "retain a report only after the native worker stops".into(),
            ));
        }
        let association = self.source_run_association(run_id.clone())?;
        source_report::retain(&self.base, &run_id, &device_id, &association, &status)
            .map_err(TrollmError::bundle)
    }

    /// Original source/install/native identity for report custody, never a new run.
    pub fn source_run_association(&self, run_id: String) -> Result<String, TrollmError> {
        history::validate_run_id(&run_id)?;
        let path = self
            .base
            .join("runs")
            .join(&run_id)
            .join("source-association.json");
        let bytes = std::fs::read(path)
            .map_err(|e| TrollmError::Io(format!("reading source association: {e}")))?;
        let source: serde_json::Value = serde_json::from_slice(&bytes)
            .map_err(|e| TrollmError::Bundle(format!("invalid source association: {e}")))?;
        if source["nativeRunId"].as_str() != Some(run_id.as_str()) {
            return Err(TrollmError::Bundle(
                "source association belongs to another run".into(),
            ));
        }
        String::from_utf8(bytes)
            .map_err(|e| TrollmError::Bundle(format!("invalid source association: {e}")))
    }

    /// Verified, pinned setup defaults for an installed flow. Legacy bundles
    /// without a descriptor return None. No credentials are part of this JSON.
    pub fn flow_setup(&self, flow_sha256: String) -> Result<Option<String>, TrollmError> {
        let setup =
            bundle::installed_flow_setup(&self.base, &flow_sha256).map_err(TrollmError::bundle)?;
        setup
            .map(|value| serde_json::to_string(&value))
            .transpose()
            .map_err(|e| TrollmError::Bundle(format!("serializing flow setup: {e}")))
    }

    /// Read verified model declarations for an installed flow, including unused
    /// slots and slots in referenced child flows. No execution is started.
    pub fn flow_has_declared_model_slots(
        &self,
        flow_sha256: String,
        source_installation_sha256: Option<String>,
    ) -> Result<bool, TrollmError> {
        bundle::flow_has_declared_model_slots(
            &self.base,
            &flow_sha256,
            source_installation_sha256.as_deref(),
        )
        .map_err(TrollmError::bundle)
    }

    /// UI admission hint: keep one native inference run active by default.
    /// Core callers may still explicitly use concurrent runs.
    pub fn has_active_run(&self) -> bool {
        self.runs.has_active()
    }

    /// Lightweight history metadata, without event-log or result-artifact reads.
    /// Missing legacy metadata leaves an openable row with unknown fields.
    pub fn run_summary(&self, run_id: String) -> Result<RunSummaryInfo, TrollmError> {
        history::validate_run_id(&run_id)?;
        let live = self.runs.get(&run_id).map(|handle| {
            let phase = handle.status.lock().unwrap();
            let active = handle.control.worker_active()
                && matches!(*phase, RunPhase::Running | RunPhase::Paused { .. });
            (phase.clone(), active)
        });
        history::read_run_summary(&self.base, &run_id, live)
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
        .map_err(TrollmError::run)?;
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
        let provider = self.token_provider.lock().unwrap().clone();
        self.start_configured(flow_sha256, user_input, configuration, provider)
    }

    /// §9: current status. Falls back to the persisted snapshot for runs not
    /// in this process (app restart), so the UI survives process death.
    pub fn poll_status(&self, run_id: String) -> Result<RunStatusInfo, TrollmError> {
        history::validate_run_id(&run_id)?;
        let (phase, worker_active, activity) = match self.runs.get(&run_id) {
            Some(handle) => {
                let snapshot = {
                    // Keep terminal publication excluded until the worker flag and
                    // activity belong to this same phase observation.
                    let phase = handle.status.lock().unwrap();
                    // Terminal status can be published before model/gateway
                    // teardown. Expose the actual worker flag through that gap.
                    let active = handle.control.worker_active();
                    let activity = if active && matches!(*phase, RunPhase::Running) {
                        handle.control.activity()
                    } else {
                        None
                    };
                    (phase.clone(), active, activity)
                };
                if !snapshot.1 && matches!(snapshot.0, RunPhase::Running | RunPhase::Paused { .. })
                {
                    // An unwound worker has the same recovery view as a cold
                    // process. File reads happen after releasing the status lock.
                    let phase = run::status_from_disk(&self.base, &run_id)
                        .map_err(TrollmError::run)?
                        .unwrap_or_else(|| RunPhase::Failed {
                            reason: "interrupted before saved progress; start a new run".into(),
                        });
                    (phase, false, None)
                } else {
                    snapshot
                }
            }
            None => (
                run::status_from_disk(&self.base, &run_id)
                    .map_err(TrollmError::run)?
                    .ok_or_else(|| TrollmError::NotFound(format!("no run '{run_id}'")))?,
                false,
                None,
            ),
        };
        let last_seq = run::last_event_seq(&self.base, &run_id).map_err(TrollmError::run)?;
        let final_report = if matches!(phase, RunPhase::Succeeded) {
            run::final_report(&self.base, &run_id).map_err(TrollmError::run)?
        } else {
            None
        };
        Ok(RunStatusInfo {
            run_id,
            phase: RunPhaseApi::from(&phase),
            worker_active,
            activity,
            last_seq,
            final_report,
        })
    }

    /// Observe a bounded live view. Once seeded, idle polls perform no journal
    /// reads. This deliberately does not validate external edits to old records;
    /// use poll_status/tail_events for strict disk validation.
    pub fn observe_run(
        &self,
        run_id: String,
        from_seq: u64,
    ) -> Result<RunObservation, TrollmError> {
        history::validate_run_id(&run_id)?;
        let _resume = self.resume_lock.lock().unwrap();
        if let Some(handle) = self.runs.get(&run_id) {
            let phase = handle.status.lock().unwrap();
            if handle.control.worker_active() {
                let active = true; // The actual worker flag above is still set.
                let activity = if active && matches!(*phase, RunPhase::Running) {
                    handle.control.activity()
                } else {
                    None
                };
                let view = handle.control.observation.lock().map_err(|_| {
                    TrollmError::Run("observation unavailable; reopen after worker stops".into())
                })?;
                if let Some(error) = &view.report_error {
                    return Err(TrollmError::Run(error.clone()));
                }
                let status = RunStatusInfo {
                    run_id,
                    phase: RunPhaseApi::from(&*phase),
                    worker_active: active,
                    activity,
                    last_seq: view.last_seq,
                    final_report: if matches!(*phase, RunPhase::Succeeded) {
                        view.final_report.clone()
                    } else {
                        None
                    },
                };
                return Ok(view.batch(status, from_seq));
            }
        }
        // Serialize cold construction so concurrent screen/service first polls
        // share one strict scan. Entries are immutable until explicit invalidation,
        // eviction or a new worker registration.
        let mut cache = self.observations.lock().unwrap();
        if cache.get(&run_id).is_none() {
            let phase = match self.runs.get(&run_id) {
                Some(handle) if handle.control.has_volatile_failure() => {
                    handle.status.lock().unwrap().clone()
                }
                _ => run::status_from_disk(&self.base, &run_id)
                    .map_err(TrollmError::run)?
                    .ok_or_else(|| TrollmError::NotFound(format!("no run '{run_id}'")))?,
            };
            let mut view =
                observation::Projection::seed(&self.base, &run_id).map_err(TrollmError::run)?;
            if matches!(phase, RunPhase::Succeeded) {
                view.final_report = run::final_report_for_node(
                    &self.base.join("runs").join(&run_id),
                    view.report_node.as_deref(),
                )
                .map_err(TrollmError::run)?;
            }
            let status = RunStatusInfo {
                run_id: run_id.clone(),
                phase: RunPhaseApi::from(&phase),
                worker_active: false,
                activity: None,
                last_seq: view.last_seq,
                final_report: view.final_report.clone(),
            };
            cache.insert(run_id.clone(), view, status);
        }
        let (view, status) = cache.get(&run_id).unwrap();
        Ok(view.batch(status.clone(), from_seq))
    }

    /// Discard an inactive derived view. The next observe_run strictly rebuilds
    /// it from disk. A live worker exclusively owns its view until it stops.
    pub fn invalidate_observation(&self, run_id: String) -> Result<(), TrollmError> {
        history::validate_run_id(&run_id)?;
        let _resume = self.resume_lock.lock().unwrap();
        if self
            .runs
            .get(&run_id)
            .is_some_and(|handle| handle.control.worker_active())
        {
            return Err(TrollmError::Run(
                "cannot invalidate a live worker observation".into(),
            ));
        }
        self.observations.lock().unwrap().remove(&run_id);
        Ok(())
    }

    /// §9: pause. Plan-paused runs stay paused; running runs get a
    /// cooperative hold at the next model/tool boundary (see `run.rs` docs).
    pub fn pause(&self, run_id: String) -> Result<(), TrollmError> {
        history::validate_run_id(&run_id)?;
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
        let provider = self.token_provider.lock().unwrap().clone();
        self.resume_using(run_id, provider)
    }

    /// §9: cancel. Cooperative boundaries, including retry backoff, record a
    /// durable cancelled outcome. The public terminal phase remains
    /// `Failed { reason: "cancelled" }`.
    pub fn cancel(&self, run_id: String) -> Result<(), TrollmError> {
        history::validate_run_id(&run_id)?;
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
        history::validate_run_id(&run_id)?;
        let (events, _) =
            run::tail_events(&self.base, &run_id, from_seq).map_err(TrollmError::run)?;
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

impl TrollmRuntime {
    /// Check native worker ownership without opening the run's saved evidence.
    /// Terminal status can be published while native teardown is still active.
    pub fn run_worker_active(&self, run_id: &str) -> bool {
        self.runs
            .get(run_id)
            .is_some_and(|handle| handle.control.worker_active())
    }

    /// Whether the runtime has a live pause or unfinished saved progress that
    /// can be explicitly resumed. Reconstruction still validates all evidence.
    pub fn run_resume_available(&self, run_id: &str) -> Result<bool, TrollmError> {
        history::validate_run_id(run_id)?;
        let _resume = self.resume_lock.lock().unwrap();
        if let Some(handle) = self.runs.get(run_id) {
            let phase = handle.status.lock().unwrap();
            if handle.control.worker_active() {
                return Ok(matches!(*phase, RunPhase::Paused { .. }));
            }
            if handle.control.has_volatile_failure() {
                return Ok(false);
            }
        }
        let dir = trollm_core::RunDir::new(self.base.join("runs").join(run_id));
        if !dir.snapshot_path().is_file() {
            return Ok(false);
        }
        run::terminal_snapshot(&self.base, run_id)
            .map(|terminal| !terminal)
            .map_err(TrollmError::run)
    }

    /// Rust embedding entry for a configured run with host-pinned credential
    /// selection. The provider is owned by this run.
    pub fn start_run_configured_with_provider(
        &self,
        flow_sha256: String,
        user_input: String,
        configuration: configuration::RunConfiguration,
        provider: Box<dyn TokenProvider>,
    ) -> Result<String, TrollmError> {
        self.start_configured(
            flow_sha256,
            user_input,
            configuration,
            Some(Arc::new(provider)),
        )
    }

    /// Rust embedding entry for explicit recovery. An existing worker retains
    /// its original provider. A reconstructed worker uses the supplied one.
    pub fn resume_with_provider(
        &self,
        run_id: String,
        provider: Box<dyn TokenProvider>,
    ) -> Result<(), TrollmError> {
        self.resume_using(run_id, Some(Arc::new(provider)))
    }

    fn start_configured(
        &self,
        flow_sha256: String,
        user_input: String,
        configuration: configuration::RunConfiguration,
        provider: Option<SharedTokenProvider>,
    ) -> Result<String, TrollmError> {
        let spawned = run::spawn_run(
            self.base.clone(),
            flow_sha256,
            user_input,
            provider,
            None,
            Some(configuration),
        )
        .map_err(TrollmError::run)?;
        let run_id = spawned.run_id.clone();
        self.runs.insert(run_id.clone(), spawned.handle);
        Ok(run_id)
    }

    fn resume_using(
        &self,
        run_id: String,
        provider: Option<SharedTokenProvider>,
    ) -> Result<(), TrollmError> {
        history::validate_run_id(&run_id)?;
        self.resume_or_reconstruct(run_id.clone(), || {
            let flow_sha = run::plan_sha_of_run(&self.base, &run_id).map_err(TrollmError::run)?;
            let user_input = std::fs::read_to_string(
                self.base.join("runs").join(&run_id).join("user_input.txt"),
            )
            .unwrap_or_default();
            let spawned = run::spawn_run(
                self.base.clone(),
                flow_sha,
                user_input,
                provider,
                Some(run_id),
                None,
            )
            .map_err(TrollmError::run)?;
            Ok(spawned.handle)
        })
    }

    /// Keep the registry check, reconstruction and insertion atomic with respect
    /// to other explicit resume requests on this runtime. The injected factory
    /// allows host tests to count reconstruction without starting model workers.
    fn resume_or_reconstruct(
        &self,
        run_id: String,
        reconstruct: impl FnOnce() -> Result<Arc<run::RunHandle>, TrollmError>,
    ) -> Result<(), TrollmError> {
        let _resume = self.resume_lock.lock().unwrap();
        if let Some(handle) = self.runs.get(&run_id) {
            if handle.control.worker_active() {
                handle.resume();
                return Ok(());
            }
            // Do not replace the only diagnostic when failure persistence was
            // impossible. Ordinary durable failures follow the disk state below.
            if handle.control.has_volatile_failure() {
                return Ok(());
            }
        }
        // Cache eviction must not turn a completed run into new initialization.
        // Unfinished saved progress can still recover from a worker failure,
        // consistently for cached, evicted and process-restarted runs.
        if run::terminal_snapshot(&self.base, &run_id).map_err(TrollmError::run)? {
            return Ok(());
        }
        let handle = reconstruct()?;
        self.observations.lock().unwrap().remove(&run_id);
        self.runs.insert(run_id, handle);
        Ok(())
    }
}
