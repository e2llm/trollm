use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, DirBuilder, File};
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::UNIX_EPOCH;

use anyhow::{ensure, Context};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use trollm_core::{artifacts::write_atomic, plan::hex_sha256};
use trollm_uniffi::configuration::{RunConfiguration, CONFIGURATION_FILE};
use trollm_uniffi::setup::ConnectionAuth;
use trollm_uniffi::{
    EventLine, RunActivityApi, RunPhaseApi, RunStatusInfo, TrollmError, TrollmRuntime,
};
use uuid::Uuid;

use crate::connections::{ConnectionPins, ConnectionStatus, Connections, PinRequest};
use crate::error::{ApiError, Result};
use crate::flows::{is_sha, read_bounded, supports_manual_archive, FlowDto, InstalledFlow};
use crate::models::{ModelManager, ModelStatus};
use crate::session::Sessions;

const STARTERS: &[&str] = &["text-summary.tar", "check-total.tar", "browser-summary.tar"];
const MAX_HISTORY: usize = 50;
const MAX_INPUT_BYTES: usize = 65_536;
const MAX_RECEIPT_BYTES: u64 = 2048;
const MAX_RUN_METADATA_BYTES: u64 = 2 * 1024 * 1024;
const RUN_IDENTITY_VERSION: u8 = 1;

pub const fn backend() -> &'static str {
    if cfg!(feature = "llama") {
        "llama"
    } else if cfg!(feature = "stub") {
        "stub"
    } else {
        "unavailable"
    }
}

pub struct Host {
    base: PathBuf,
    receipts: PathBuf,
    run_identities: PathBuf,
    runtime: Arc<TrollmRuntime>,
    pub(crate) connections: Arc<Connections>,
    pub(crate) models: Arc<ModelManager>,
    pub(crate) sessions: Sessions,
    pub(crate) login_slot: Arc<tokio::sync::Semaphore>,
    pub(crate) import_slot: Arc<tokio::sync::Semaphore>,
    // Start and Resume share one admission gate. Keep the last worker's ID
    // through terminal publication until its native teardown acknowledges exit.
    current_run: Mutex<Option<String>>,
    _owner: File,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FlowDetails {
    flow: FlowDto,
    can_start: bool,
    #[serde(flatten)]
    readiness: Readiness,
}

#[derive(Serialize)]
struct Readiness {
    models: Vec<ModelStatus>,
    connections: Vec<ConnectionStatus>,
    blockers: Vec<&'static str>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RunDto {
    id: String,
    flow_sha256: String,
    phase: Phase,
    worker_active: bool,
    can_resume: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    report: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    activity: Option<String>,
    last_seq: u64,
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Debug)]
#[serde(rename_all = "lowercase")]
enum Phase {
    Running,
    Paused,
    Interrupted,
    Succeeded,
    Failed,
}

#[derive(Serialize)]
pub(crate) struct EventDto {
    seq: u64,
    #[serde(rename = "type")]
    event_type: String,
    text: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RunDetails {
    pub(crate) run: RunDto,
    events: Vec<EventDto>,
    has_more: bool,
    next_seq: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct StartRequest {
    request_id: String,
    flow_sha256: String,
    input: String,
}

#[derive(Serialize)]
pub(crate) struct StartResponse {
    id: String,
    phase: Phase,
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum RunAction {
    Pause,
    Resume,
    Cancel,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Receipt {
    fingerprint: String,
    state: ReceiptState,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RunIdentity {
    version: u8,
    flow_sha256: String,
    connections: ConnectionPins,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "camelCase")]
enum ReceiptState {
    Pending,
    Started { id: String },
}

fn valid_uuid(value: &str) -> bool {
    value.len() == 36 && Uuid::parse_str(value).is_ok_and(|id| id.to_string() == value)
}

fn read_receipt(path: &Path) -> anyhow::Result<Option<Receipt>> {
    match path.try_exists()? {
        true => Ok(Some(serde_json::from_slice(&read_bounded(
            path,
            MAX_RECEIPT_BYTES,
        )?)?)),
        false => Ok(None),
    }
}

pub(crate) fn start_uncertain() -> ApiError {
    ApiError::new(
        StatusCode::CONFLICT,
        "START_UNCERTAIN",
        "Start may already have reached the runtime. Check History before starting again.",
    )
}

fn invalid_run_action() -> ApiError {
    ApiError::new(
        StatusCode::CONFLICT,
        "RUN_ACTION_UNAVAILABLE",
        "This action is unavailable in the current run state.",
    )
}

fn missing_run_identity() -> ApiError {
    ApiError::new(
        StatusCode::CONFLICT,
        "RUN_IDENTITY_UNAVAILABLE",
        "This run cannot resume because its original settings are unavailable.",
    )
}

fn occupied() -> ApiError {
    ApiError::new(
        StatusCode::CONFLICT,
        "RUN_ACTIVE",
        "Another run is still active.",
    )
}

fn unsupported_profile() -> ApiError {
    ApiError::new(
        StatusCode::UNPROCESSABLE_ENTITY,
        "UNSUPPORTED_PROFILE",
        "This flow requires an integration unavailable on desktop.",
    )
}

impl Host {
    /// Open a private directory, acquire its process-lifetime lock, and verify
    /// bundled starters. Call from a synchronous thread.
    pub fn open(
        data_dir: &Path,
        starter_dir: &Path,
        callback_url: String,
    ) -> anyhow::Result<Arc<Self>> {
        DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(data_dir)?;
        let data_dir = data_dir.canonicalize()?;
        ensure!(
            fs::metadata(&data_dir)?.permissions().mode() & 0o077 == 0,
            "desktop data directory must be private (mode 0700)",
        );
        use rustix::fs::{flock, open, FlockOperation, Mode, OFlags};
        let owner = File::from(open(
            data_dir.join(".desktop.lock"),
            OFlags::CREATE | OFlags::RDWR | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::RUSR | Mode::WUSR,
        )?);
        ensure!(
            owner.metadata()?.is_file(),
            "invalid desktop directory lock"
        );
        flock(&owner, FlockOperation::NonBlockingLockExclusive)
            .context("acquiring desktop data directory ownership")?;

        let base = data_dir.join("trollm");
        let receipts = data_dir.join("start-requests");
        let run_identities = data_dir.join("run-identities");
        fs::create_dir_all(&receipts)?;
        fs::create_dir_all(&run_identities)?;
        File::open(&data_dir)?.sync_all()?;
        let connections = Connections::open(&data_dir, callback_url)?;
        let models = ModelManager::new(&data_dir)?;
        let runtime = TrollmRuntime::new(
            data_dir
                .to_str()
                .context("desktop data directory must use UTF-8")?
                .to_owned(),
            None,
        )?;

        for name in STARTERS {
            let bytes = read_bounded(
                &starter_dir.join(name),
                trollm_uniffi::tar::MAX_BUNDLE_BYTES,
            )
            .with_context(|| format!("reading packaged starter {name}"))?;
            runtime
                .install_flow_bundle(bytes)
                .map_err(|_| anyhow::anyhow!("packaged starter {name} failed validation"))?;
        }
        Ok(Arc::new(Self {
            base,
            receipts,
            run_identities,
            runtime,
            connections,
            models,
            sessions: Sessions::default(),
            login_slot: Arc::new(tokio::sync::Semaphore::new(1)),
            import_slot: Arc::new(tokio::sync::Semaphore::new(1)),
            current_run: Mutex::default(),
            _owner: owner,
        }))
    }

    pub(crate) fn load_flow(&self, sha: &str) -> Result<InstalledFlow> {
        if !is_sha(sha) {
            return Err(ApiError::flow_not_found());
        }
        if !self.base.join("bundles").join(sha).is_dir() {
            return Err(ApiError::flow_not_found());
        }
        let flow = InstalledFlow::load(&self.base, sha).map_err(|_| ApiError::storage())?;
        if !flow.supported() {
            return Err(unsupported_profile());
        }
        Ok(flow)
    }

    pub(crate) fn flows(&self) -> Result<Vec<FlowDto>> {
        let flows = self.runtime.list_flows().map_err(|_| ApiError::storage())?;
        Ok(flows
            .into_iter()
            .filter_map(|flow| self.load_flow(&flow.sha256).ok())
            .map(|flow| flow.info)
            .collect())
    }

    pub(crate) fn import(&self, bytes: Vec<u8>) -> Result<FlowDto> {
        let rejected = || {
            ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY, "BUNDLE_REJECTED",
            "The archive is not a valid installable flow bundle, or its flow conflicts with an installed bundle.",
        )
        };
        if !supports_manual_archive(&bytes).map_err(|_| rejected())? {
            return Err(unsupported_profile());
        }
        let installed = self
            .runtime
            .install_flow_bundle(bytes)
            .map_err(|_| rejected())?;
        Ok(self.load_flow(&installed.flow_sha256)?.info)
    }

    pub(crate) fn flow_details(&self, sha: &str) -> Result<FlowDetails> {
        let flow = self.load_flow(sha)?;
        let mut readiness = self.readiness(&flow)?;
        let current = self.current_run.lock().map_err(|_| ApiError::storage())?;
        if self.worker_active(current.as_deref()) {
            readiness.blockers.push("Another run is still active.");
        }
        Ok(FlowDetails {
            flow: flow.info,
            can_start: readiness.blockers.is_empty(),
            readiness,
        })
    }

    fn readiness(&self, flow: &InstalledFlow) -> Result<Readiness> {
        let (uses_models, servers) = flow.declarations().map_err(|_| ApiError::storage())?;
        let mut blockers = Vec::new();
        let mut models = Vec::new();
        let mut connections = Vec::new();
        if let Some(setup) = &flow.info.setup {
            models = self.models.status(setup).map_err(|_| ApiError::storage())?;
        }
        if uses_models {
            if backend() == "unavailable" {
                blockers.push("This build has no model backend.");
            }
            if flow.info.setup.is_none() {
                blockers.push("This flow has no pinned model downloads in its setup.");
            } else if models.iter().any(|model| model.status != "ready") {
                blockers.push("Download and verify the required models.");
            }
        }
        for (name, url) in servers {
            let declared = flow
                .info
                .setup
                .as_ref()
                .and_then(|setup| setup.connections.get(name));
            let status = self
                .connections
                .status(name, url, declared.map(|decl| decl.auth))
                .map_err(|_| ApiError::storage())?;
            if declared.is_some_and(|decl| {
                matches!(decl.auth, ConnectionAuth::Oauth)
                    && decl
                        .issuer_url
                        .as_deref()
                        .is_some_and(|issuer| !supports_issuer(issuer, &status.url))
            }) {
                blockers.push("This flow requires an OAuth issuer unavailable on desktop.");
            }
            connections.push(status);
        }
        if connections.iter().any(|item| !item.connected) {
            blockers.push("Set up the required connections.");
        }
        Ok(Readiness {
            models,
            connections,
            blockers,
        })
    }

    fn worker_active(&self, current: Option<&str>) -> bool {
        current.is_some_and(|id| self.runtime.run_worker_active(id))
            || self.runtime.has_active_run()
    }

    pub(crate) fn start(&self, request: StartRequest) -> Result<StartResponse> {
        if !valid_uuid(&request.request_id) || !is_sha(&request.flow_sha256) {
            return Err(ApiError::invalid_request());
        }
        if request.input.len() > MAX_INPUT_BYTES {
            return Err(ApiError::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                "INVALID_INPUT",
                "Input exceeds 64 KiB.",
            ));
        }
        let fingerprint =
            hex_sha256(&[request.flow_sha256.as_bytes(), request.input.as_bytes()].concat());
        let mut current = self.current_run.lock().map_err(|_| start_uncertain())?;
        let receipt_path = self.receipts.join(&request.request_id);
        if let Some(receipt) = read_receipt(&receipt_path).map_err(|_| start_uncertain())? {
            if receipt.fingerprint != fingerprint {
                return Err(ApiError::new(
                    StatusCode::CONFLICT,
                    "REQUEST_ID_CONFLICT",
                    "This request ID already belongs to a different Start request.",
                ));
            }
            return match receipt.state {
                ReceiptState::Pending => Err(start_uncertain()),
                ReceiptState::Started { id } if valid_uuid(&id) => {
                    let phase = self
                        .runtime
                        .observe_run(id.clone(), u64::MAX)
                        .map_or(Phase::Interrupted, |view| {
                            phase_of(&view.status.phase, view.status.worker_active)
                        });
                    Ok(StartResponse { id, phase })
                }
                ReceiptState::Started { .. } => Err(start_uncertain()),
            };
        }
        if self.worker_active(current.as_deref()) {
            return Err(occupied());
        }
        let flow = self.load_flow(&request.flow_sha256)?;
        if let Some(input) = flow
            .info
            .setup
            .as_ref()
            .and_then(|setup| setup.input.as_ref())
        {
            input.validate_input(&request.input).map_err(|_| {
                ApiError::new(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "INVALID_INPUT",
                    "Input does not meet the flow's required fields, types or length limits.",
                )
            })?;
        }
        let readiness = self.readiness(&flow)?;
        let not_ready = || {
            ApiError::new(
                StatusCode::CONFLICT,
                "FLOW_NOT_READY",
                "Prepare the flow before starting it.",
            )
        };
        if !readiness.blockers.is_empty() {
            return Err(not_ready());
        }
        let models = match &flow.info.setup {
            Some(setup) => self
                .models
                .resolved_models(setup)
                .map_err(|_| not_ready())?,
            None => BTreeMap::new(),
        };
        let (_, servers) = flow.declarations().map_err(|_| ApiError::storage())?;
        let requirements = servers
            .into_iter()
            .map(|(name, default_url)| PinRequest {
                name,
                default_url,
                required_auth: flow
                    .info
                    .setup
                    .as_ref()
                    .and_then(|setup| setup.connections.get(name))
                    .map(|connection| connection.auth),
            })
            .collect::<Vec<_>>();
        let pins = self
            .connections
            .capture_pins(&requirements)
            .map_err(|_| not_ready())?;
        let connections = pins.resolved_connections();
        if flow.info.setup.as_ref().is_some_and(|setup| {
            setup.connections.iter().any(|(name, declared)| {
                matches!(declared.auth, ConnectionAuth::Oauth)
                    && declared.issuer_url.as_deref().is_some_and(|issuer| {
                        !connections
                            .get(name)
                            .is_some_and(|connection| supports_issuer(issuer, &connection.url))
                    })
            })
        }) {
            return Err(not_ready());
        }
        let mut configuration = RunConfiguration {
            version: 1,
            models,
            connections,
        };
        configuration
            .validate_and_resolve(&flow.plan)
            .map_err(|_| not_ready())?;
        pins.validate_endpoints(&configuration.connections)
            .map_err(|_| not_ready())?;
        let identity = serde_json::to_vec(&RunIdentity {
            version: RUN_IDENTITY_VERSION,
            flow_sha256: request.flow_sha256.clone(),
            connections: pins.clone(),
        })
        .map_err(|_| ApiError::storage())?;
        if identity.len() as u64 > MAX_RUN_METADATA_BYTES {
            return Err(not_ready());
        }
        let provider = self.connections.pinned_provider(pins);

        self.write_receipt(
            &request.request_id,
            &Receipt {
                fingerprint: fingerprint.clone(),
                state: ReceiptState::Pending,
            },
        )
        .map_err(|_| start_uncertain())?;
        let id = self
            .runtime
            .start_run_configured_with_provider(
                request.flow_sha256,
                request.input,
                configuration,
                provider,
            )
            .map_err(|_| start_uncertain())?;
        *current = Some(id.clone());
        write_atomic(&self.run_identities, &id, &identity).map_err(|_| start_uncertain())?;
        self.write_receipt(
            &request.request_id,
            &Receipt {
                fingerprint,
                state: ReceiptState::Started { id: id.clone() },
            },
        )
        .map_err(|_| start_uncertain())?;

        Ok(StartResponse {
            id,
            phase: Phase::Running,
        })
    }

    fn write_receipt(&self, key: &str, receipt: &Receipt) -> anyhow::Result<()> {
        write_atomic(&self.receipts, key, &serde_json::to_vec(receipt)?)?;
        Ok(())
    }

    fn run_path(&self, id: &str) -> Result<PathBuf> {
        if !valid_uuid(id) {
            return Err(ApiError::run_not_found());
        }
        let path = self.base.join("runs").join(id);
        if !path.is_dir() {
            return Err(ApiError::run_not_found());
        }
        if path.join("source-association.json").exists() || path.join("installation.json").exists()
        {
            return Err(unsupported_profile());
        }
        Ok(path)
    }

    fn recorded_identity(&self, id: &str) -> Result<RunIdentity> {
        let bytes = read_bounded(&self.run_identities.join(id), MAX_RUN_METADATA_BYTES)
            .map_err(|_| missing_run_identity())?;
        let identity: RunIdentity =
            serde_json::from_slice(&bytes).map_err(|_| missing_run_identity())?;
        if identity.version != RUN_IDENTITY_VERSION || !is_sha(&identity.flow_sha256) {
            return Err(missing_run_identity());
        }
        Ok(identity)
    }

    fn run_identity(&self, id: &str) -> String {
        if let Ok(identity) = self.recorded_identity(id) {
            return identity.flow_sha256;
        }
        self.runtime
            .run_summary(id.to_owned())
            .ok()
            .and_then(|summary| summary.flow_sha256)
            .unwrap_or_default()
    }

    pub(crate) fn history(&self) -> Result<Vec<RunDto>> {
        let mut newest = BTreeSet::new();
        let entries = fs::read_dir(self.base.join("runs")).map_err(|_| ApiError::storage())?;
        for entry in entries.flatten() {
            let Ok(id) = entry.file_name().into_string() else {
                continue;
            };
            if self.run_path(&id).is_err() {
                continue;
            }
            let time = entry
                .metadata()
                .ok()
                .and_then(|metadata| metadata.created().or_else(|_| metadata.modified()).ok())
                .unwrap_or(UNIX_EPOCH);
            newest.insert((time, id));
            if newest.len() > MAX_HISTORY {
                newest.pop_first();
            }
        }
        let runs = newest
            .into_iter()
            .rev()
            .filter_map(|(_, id)| {
                let summary = self.runtime.run_summary(id.clone()).ok()?;
                Some(
                    self.run_dto(
                        RunStatusInfo {
                            run_id: id.clone(),
                            phase: summary.phase.unwrap_or(RunPhaseApi::Running),
                            worker_active: self.runtime.run_worker_active(&id),
                            activity: None,
                            last_seq: 0,
                            final_report: None,
                        },
                        summary
                            .flow_sha256
                            .unwrap_or_else(|| self.run_identity(&id)),
                    ),
                )
            })
            .collect();
        Ok(runs)
    }

    pub(crate) fn observe(&self, id: &str, after: u64) -> Result<RunDetails> {
        let path = self.run_path(id)?;
        let view = match self.runtime.observe_run(id.to_owned(), after) {
            Ok(view) => view,
            Err(TrollmError::NotFound(_)) if !path.join("state.snapshot.json").exists() => {
                return Ok(RunDetails {
                    run: self.run_dto(
                        RunStatusInfo {
                            run_id: id.into(),
                            phase: RunPhaseApi::Running,
                            worker_active: false,
                            activity: None,
                            last_seq: 0,
                            final_report: None,
                        },
                        self.run_identity(id),
                    ),
                    events: Vec::new(),
                    has_more: false,
                    next_seq: after,
                });
            }
            Err(_) => return Err(ApiError::storage()),
        };
        let mut events =
            Vec::with_capacity(view.events.len() + usize::from(view.older_events_omitted));
        if view.older_events_omitted {
            let sequence = view
                .events
                .first()
                .map_or(view.next_seq, |event| event.seq.saturating_sub(1));
            events.push(EventDto {
                seq: sequence,
                event_type: "events_omitted".into(),
                text: "The oldest events are outside the retained progress window.".into(),
            });
        }
        events.extend(view.events.into_iter().map(map_event));
        Ok(RunDetails {
            run: self.run_dto(view.status, self.run_identity(id)),
            events,
            has_more: view.has_more,
            next_seq: view.next_seq,
        })
    }

    pub(crate) fn action(&self, id: &str, action: RunAction) -> Result<RunDto> {
        let mut current = self.current_run.lock().map_err(|_| ApiError::storage())?;
        let run = self.observe(id, u64::MAX)?.run;
        match action {
            RunAction::Pause if run.worker_active && run.phase == Phase::Running => {
                self.runtime
                    .pause(id.into())
                    .map_err(|_| invalid_run_action())?;
            }
            RunAction::Resume if may_resume_phase(run.phase) => {
                if current.as_deref() != Some(id) && self.worker_active(current.as_deref()) {
                    return Err(occupied());
                }
                self.resume(id)?;
                *current = Some(id.into());
            }
            RunAction::Cancel
                if run.worker_active && matches!(run.phase, Phase::Running | Phase::Paused) =>
            {
                self.runtime
                    .cancel(id.into())
                    .map_err(|_| invalid_run_action())?;
            }
            _ => return Err(invalid_run_action()),
        }
        Ok(self.observe(id, u64::MAX)?.run)
    }

    fn run_dto(&self, status: RunStatusInfo, flow_sha256: String) -> RunDto {
        let phase = phase_of(&status.phase, status.worker_active);
        let can_resume = may_resume_phase(phase)
            && self
                .runtime
                .run_resume_available(&status.run_id)
                .unwrap_or(false)
            && self.resume_identity(&status.run_id).is_ok();
        map_run(status, flow_sha256, phase, can_resume)
    }

    fn resume_identity(&self, id: &str) -> Result<RunIdentity> {
        let identity = self.recorded_identity(id)?;
        let summary = self
            .runtime
            .run_summary(id.to_owned())
            .map_err(|_| missing_run_identity())?;
        let identity_matches = match summary.flow_sha256.as_deref() {
            Some(sha) => sha == identity.flow_sha256.as_str(),
            // A cooperative pause can happen before the first snapshot. The
            // existing worker and its configured identity already belong together.
            None => self.runtime.run_worker_active(id),
        };
        if !identity_matches {
            return Err(missing_run_identity());
        }
        let path = self.run_path(id)?.join(CONFIGURATION_FILE);
        let bytes =
            read_bounded(&path, MAX_RUN_METADATA_BYTES).map_err(|_| missing_run_identity())?;
        let configuration: RunConfiguration =
            serde_json::from_slice(&bytes).map_err(|_| missing_run_identity())?;
        identity
            .connections
            .validate_endpoints(&configuration.connections)
            .map_err(|_| missing_run_identity())?;
        self.connections
            .validate_pins(&identity.connections)
            .map_err(|_| {
                ApiError::new(
                    StatusCode::CONFLICT,
                    "RUN_CREDENTIALS_CHANGED",
                    "This run cannot resume because its original connection credentials changed.",
                )
            })?;
        Ok(identity)
    }

    fn resume(&self, id: &str) -> Result<()> {
        if !self
            .runtime
            .run_resume_available(id)
            .map_err(|_| invalid_run_action())?
        {
            return Err(invalid_run_action());
        }
        let identity = self.resume_identity(id)?;
        self.runtime
            .resume_with_provider(
                id.to_owned(),
                self.connections.pinned_provider(identity.connections),
            )
            .map_err(|_| invalid_run_action())
    }
}

fn supports_issuer(issuer: &str, endpoint: &str) -> bool {
    match (url::Url::parse(issuer), url::Url::parse(endpoint)) {
        (Ok(issuer), Ok(endpoint)) => {
            issuer.origin() == endpoint.origin()
                && issuer.path() == "/"
                && issuer.query().is_none()
                && issuer.fragment().is_none()
        }
        _ => false,
    }
}

fn phase_of(phase: &RunPhaseApi, worker_active: bool) -> Phase {
    match phase {
        RunPhaseApi::Running if !worker_active => Phase::Interrupted,
        RunPhaseApi::Running => Phase::Running,
        RunPhaseApi::Paused { .. } => Phase::Paused,
        RunPhaseApi::Succeeded => Phase::Succeeded,
        RunPhaseApi::Failed { .. } => Phase::Failed,
    }
}

fn may_resume_phase(phase: Phase) -> bool {
    matches!(phase, Phase::Paused | Phase::Interrupted | Phase::Failed)
}

fn map_run(status: RunStatusInfo, flow_sha256: String, phase: Phase, can_resume: bool) -> RunDto {
    let error = match status.phase {
        RunPhaseApi::Failed { reason } if reason == "cancelled" => Some("Cancelled."),
        RunPhaseApi::Failed { reason }
            if reason == "interrupted before saved progress; start a new run" =>
        {
            Some("The worker stopped before saving progress.")
        }
        RunPhaseApi::Failed { .. } => {
            Some("The runtime ended this run without a completed report.")
        }
        _ => None,
    };
    let activity = status.activity.map(|activity| match activity {
        RunActivityApi::Starting => "Starting runtime".into(),
        RunActivityApi::VerifyingModel { slot } => format!("Verifying model ({slot})"),
        RunActivityApi::LoadingModel { slot } => format!("Loading model ({slot})"),
        RunActivityApi::Generating { slot } => format!("Generating ({slot})"),
    });
    RunDto {
        id: status.run_id,
        flow_sha256,
        phase,
        worker_active: status.worker_active,
        can_resume,
        report: status.final_report,
        error,
        activity,
        last_seq: status.last_seq,
    }
}

fn map_event(event: EventLine) -> EventDto {
    // The journal may contain tool arguments, endpoint descriptions and model
    // text. Expose progress using its validated type and authored node IDs.
    let text = match event.event_type.as_str() {
        "node_entered" => {
            let node = serde_json::from_str::<serde_json::Value>(&event.json)
                .ok()
                .and_then(|value| {
                    value["node"]
                        .as_str()
                        .filter(|id| {
                            id.len() <= 128
                                && id.bytes().all(|b| {
                                    b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.')
                                })
                        })
                        .map(str::to_owned)
                });
            node.map_or_else(
                || "Entered step".into(),
                |node| format!("Entered step {node}"),
            )
        }
        kind => {
            let mut text = kind.replace('_', " ");
            if let Some(first) = text.get_mut(..1) {
                first.make_ascii_uppercase();
            }
            text
        }
    };
    EventDto {
        seq: event.seq,
        event_type: event.event_type,
        text,
    }
}
