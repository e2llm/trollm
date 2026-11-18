//! Installed selected-profile composition on the existing ToolGateway port.
//! Model/task JSON cannot supply these native authority and clock checks.
use crate::collection::{raw_sha256, retained_outcome_request, SelectionRecord};
use anyhow::{ensure, Context, Result};
use base64::Engine;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use trollm_mcp::clock::parse_selected_utc_ms;
use trollm_mcp::preparation::{
    CallPreparer, PreparationContext, PreparationEvidence, PreparedCall,
};
use trollm_mcp::private_guard::{GuardRequest, PrivateGuardClient};
use trollm_mcp::{ToolCall, ToolResult};

/// Implemented by the native installation, never deserialized from a flag/hash.
/// Both methods must fail when their actual authority/device evidence is absent.
pub trait InstalledSelectedEnvironment: Send + Sync {
    fn start_clock_maintenance(&self) -> Result<()> {
        Ok(())
    }
    fn stop_clock_maintenance(&self) {}

    fn check_current_authority(
        &self,
        selection: &SelectionRecord,
        context: &PreparationContext,
    ) -> Result<()>;
    /// Same-device conservative UTC upper bound, including actual error and
    /// suspend/discontinuity checks. Host coverage/Android offsets cannot supply it.
    fn current_utc_upper_ms(&self) -> Result<u64>;
    /// Bounded device-local authority/revocation and clock check only, using
    /// installed evidence acquired earlier. No filesystem/network/refresh.
    /// Missing actual platform implementation refuses rather than delegating to
    /// the potentially blocking acquisition methods above.
    fn local_write_utc_upper_ms(
        &self,
        _selection: &SelectionRecord,
        _context: &PreparationContext,
    ) -> Result<u64> {
        anyhow::bail!("selected local authority/device-clock check unavailable")
    }
}

#[derive(Clone)]
pub struct ActionBinding {
    pub plan_pin: String,
    pub node: String,
    pub task_ref: String,
    pub purpose: String,
    pub cursor_node: Option<String>,
    pub result_artifact: String,
}

/// Out-of-band native installation inputs, never decoded from model/flow JSON.
pub struct SelectedRunInstallation {
    ready: Value,
    client: PrivateGuardClient,
    selected_device_max_error_us: u64,
    calibrated_clock_policy: Option<crate::collection_clock::CalibratedClockPolicy>,
    expected_static_key_sha256: String,
    private_installation_key: Option<String>,
}
impl SelectedRunInstallation {
    pub(crate) fn validate_configuration(
        configuration: &crate::configuration::RunConfiguration,
        endpoint: &str,
    ) -> Result<()> {
        let coder = configuration
            .models
            .get("coder")
            .context("selected coder configuration unavailable")?;
        ensure!(
            configuration.models.len() == 1
                && coder.sha256
                    == "bd258782e35f7f458f8aced1adc053e6e92e89bc735ba3be89d38a06121dc517"
                && coder.ctx == 4096
                && coder.temperature == 0.2
                && coder.top_p == 0.95,
            "selected model/runtime configuration differs from admitted member profile"
        );
        ensure!(
            configuration.connections.len() == 1
                && configuration
                    .connections
                    .get("e2llm")
                    .is_some_and(|value| value.url == endpoint),
            "selected runtime endpoint differs from inspected member profile"
        );
        Ok(())
    }
    pub fn new(
        ready: Value,
        client: PrivateGuardClient,
        selected_device_max_error_us: u64,
        expected_static_key_sha256: String,
    ) -> Result<Self> {
        ensure!(
            selected_device_max_error_us > 0,
            "selected device error policy unavailable"
        );
        ensure!(
            expected_static_key_sha256.len() == 64
                && expected_static_key_sha256
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
            "installed private key binding unavailable"
        );
        Ok(Self {
            ready,
            client,
            selected_device_max_error_us,
            calibrated_clock_policy: None,
            expected_static_key_sha256,
            private_installation_key: None,
        })
    }
    pub(crate) fn new_calibrated(
        ready: Value,
        client: PrivateGuardClient,
        policy: crate::collection_clock::CalibratedClockPolicy,
        expected_static_key_sha256: String,
    ) -> Result<Self> {
        policy.validate()?;
        ensure!(
            expected_static_key_sha256.len() == 64
                && expected_static_key_sha256
                    .bytes()
                    .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c)),
            "installed private key binding unavailable"
        );
        Ok(Self {
            ready,
            client,
            selected_device_max_error_us: 0,
            calibrated_clock_policy: Some(policy),
            expected_static_key_sha256,
            private_installation_key: None,
        })
    }
    pub(crate) fn with_private_installation_key(mut self, key: String) -> Self {
        self.private_installation_key = Some(key);
        self
    }
    pub(crate) fn bind(
        self,
        base: &Path,
        run_directory: &Path,
        association: &Value,
        inspected: &crate::collection_members::InspectedCollectionMembers,
        provider: crate::SharedTokenProvider,
        endpoint: &str,
    ) -> Result<Arc<InstalledCollectionPreparer>> {
        let mut stage = "SELECTED_BIND_STATIC_KEY_MODE";
        let result = (|| -> Result<Arc<InstalledCollectionPreparer>> {
            ensure!(
                provider.connection_auth("e2llm".into(), endpoint.into())
                    == crate::ConnectionAuth::Bearer,
                "selected installation requires original static-key mode"
            );
            stage = "SELECTED_BIND_STATIC_KEY_READ";
            let key = provider
                .bearer_token("e2llm".into(), endpoint.into())
                .context("selected installation key unavailable")?;
            stage = "SELECTED_BIND_STATIC_KEY_HASH";
            ensure!(
                raw_sha256(key.as_bytes()) == self.expected_static_key_sha256,
                "selected installation key differs from private adapter binding"
            );
            drop(key);
            stage = "SELECTED_BIND_SOURCE_SELECTION";
            let source: crate::collection_members::CollectionSource =
                serde_json::from_value(association["descriptor"]["source"].clone())?;
            let directory = crate::bundle::revision_dir(base, &source.installation_sha256)?;
            let raw = crate::setup::read_input_evidence(&directory, "selection.json", 65536)?;
            let selection = SelectionRecord::decode(&raw, &source.selection_ref, &source.owner)?;
            let suffix = selection.selection.items.len()
                - selection.selection.continuation.prefix_length as usize;
            let matched = crate::collection_members::is_matched_profile(&inspected.member_profile);
            ensure!(
                source.profile_id == inspected.member_profile && (matched || suffix <= 1),
                "selected multi-page navigation installation unavailable"
            );
            ensure!(
                association["nativeRunId"].as_str()
                    == run_directory.file_name().and_then(|v| v.to_str()),
                "selected installation native run mismatch"
            );
            stage = "SELECTED_BIND_AUTHORITY_AGE_POLICY";
            let age = self.ready["preparationPolicy"]["maxObservationAgeMs"]
                .as_u64()
                .filter(|v| *v > 0)
                .context("selected authority age policy unavailable")?;
            let environment = Arc::new(if let Some(policy) = self.calibrated_clock_policy {
                let publication_cutoff = parse_selected_utc_ms(
                    association["descriptor"]["expiresAt"]
                        .as_str()
                        .context("original publication expiry unavailable")?,
                )?;
                stage = "SELECTED_BIND_CLOCK_CONSTRUCTION";
                let clock = crate::collection_clock::CalibratedClock::new(
                    policy,
                    self.client.clone(),
                    &self.ready,
                    &selection,
                    publication_cutoff,
                    run_directory.to_path_buf(),
                )?;
                stage = "SELECTED_BIND_SOURCE_AUTHORITY_ASSOCIATION";
                crate::collection_authority::ApplicationSelectedEnvironment::new_calibrated(
                    provider,
                    association,
                    &selection,
                    age,
                    clock,
                )?
            } else {
                stage = "SELECTED_BIND_KERNEL_CLOCK_AUTHORITY";
                crate::collection_authority::ApplicationSelectedEnvironment::new(
                    provider,
                    association,
                    &selection,
                    age,
                    self.selected_device_max_error_us,
                )?
            });
            stage = "SELECTED_BIND_READY_ACTION_SNAPSHOT";
            let target = ("e2llm".into(), "act".into());
            let mut preparer = if matched {
                InstalledCollectionPreparer::new_matched(
                    selection,
                    self.ready,
                    inspected,
                    target,
                    run_directory.to_path_buf(),
                    self.client,
                    environment.clone(),
                )?
            } else {
                InstalledCollectionPreparer::new(
                    selection,
                    self.ready,
                    inspected.bindings.clone(),
                    target,
                    run_directory.to_path_buf(),
                    self.client,
                    environment.clone(),
                )?
                .with_inspected_snapshot(inspected)?
            };
            preparer.expected_static_key_sha256 = Some(self.expected_static_key_sha256);
            preparer.private_installation_key = self.private_installation_key.clone();
            // Empty runs require actual source and actor checks without inventing a
            // native tool-call context for startup authorization.
            stage = "SELECTED_BIND_INITIAL_CLOCK_AUTHORITY";
            let now = environment.authorize_run_start()?;
            stage = "SELECTED_BIND_ACTOR_DEADLINE";
            let expiry = preparer.ready["expiresAt"]
                .as_str()
                .context("selected actor expiry unavailable")?;
            let cutoff = parse_selected_utc_ms(expiry)?;
            ensure!(now < cutoff, "selected installation expired");
            let deadline = Instant::now()
                .checked_add(Duration::from_millis(cutoff - now))
                .context("selected installation deadline overflow")?;
            stage = "SELECTED_BIND_ACTOR_CONTROL";
            let control = preparer.client.control(deadline, &|| {
                ensure!(
                    environment.current_utc_upper_ms()? < cutoff,
                    "selected installation expired"
                );
                Ok(())
            })?;
            stage = "SELECTED_BIND_ACTOR_ASSOCIATION";
            ensure!(
                !control.aborted
                    && control.invocation_key == preparer.selection.selection.invocation_key
                    && control.expires_at == expiry,
                "selected installer actor association changed"
            );
            stage = "SELECTED_BIND_FINAL_CLOCK_AUTHORITY";
            environment.authorize_run_start()?;
            ensure!(
                environment.current_utc_upper_ms()? < cutoff,
                "selected installation expired after actor read"
            );
            stage = "SELECTED_BIND_PRIVATE_BINDING_CUSTODY";
            if let Some(key) = self.private_installation_key {
                let bytes = serde_json::to_vec(&json!({
                    "schema":"trollm.selected-private-installation-binding.v1",
                    "privateInstallationSha256":key,
                    "nativeRunId":association["nativeRunId"],
                    "invocationKey":preparer.selection.selection.invocation_key,
                }))?;
                trollm_core::artifacts::write_atomic(
                    run_directory,
                    "selected-private-installation.json",
                    &bytes,
                )?;
            }
            stage = "SELECTED_BIND_INVOCATION_CUSTODY";
            retain_selected_run_claim(base, run_directory, association, &preparer.selection)?;
            Ok(Arc::new(preparer))
        })();
        if let Err(error) = &result {
            // Best-effort diagnostic custody cannot mask the original refusal
            // or assert worker inactivity. No execution is retried here.
            let _ = crate::selected_start::retain_failure(
                run_directory,
                crate::selected_start::closed_code(error, stage),
            );
        }
        crate::selected_start::at(result, stage, run_directory)
    }
}

fn retain_selected_run_claim(
    base: &Path,
    run_directory: &Path,
    association: &Value,
    selection: &SelectionRecord,
) -> Result<()> {
    use std::io::Write;
    let identity = json!({"owner":selection.selection.owner,"invocationKey":selection.selection.invocation_key});
    let key = raw_sha256(&serde_json::to_vec(&identity)?);
    let index = base.join("selected-invocations");
    std::fs::create_dir_all(&index)?;
    ensure!(
        std::fs::symlink_metadata(&index)?.is_dir(),
        "selected invocation index unavailable"
    );
    let claim = json!({"schema":"trollm.selected-run-claim.v1","identity":identity,
        "selectionRef":selection.selection_ref,"selectionRawSha256":raw_sha256(&selection.original_bytes),
        "nativeRunId":association["nativeRunId"],"installationSha256":association["installation"]["installationSha256"]});
    let bytes = serde_json::to_vec(&claim)?;
    let mut options = std::fs::OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(index.join(format!("{key}.json"))).context(
        "selected invocation already claimed or index unavailable; fresh execution refused",
    )?;
    // Interrupted claims remain fenced, not restartable by filename.
    file.write_all(&bytes)?;
    file.sync_all()?;
    std::fs::File::open(&index)?.sync_all()?;
    std::fs::File::open(base)?.sync_all()?;
    trollm_core::artifacts::write_atomic(run_directory, "selected-invocation.json", &bytes)?;
    Ok(())
}

pub struct InstalledCollectionPreparer {
    selection: SelectionRecord,
    ready: Value,
    bindings: Vec<ActionBinding>,
    expected_snapshot: Option<Vec<Value>>,
    inspected_root_pin: Option<String>,
    server: String,
    tool: String,
    run_directory: PathBuf,
    client: PrivateGuardClient,
    environment: Arc<dyn InstalledSelectedEnvironment>,
    actor_cutoff_ms: u64,
    expected_static_key_sha256: Option<String>,
    private_installation_key: Option<String>,
    intents: std::sync::Mutex<std::collections::BTreeMap<String, (u64, String, String)>>,
}
impl InstalledCollectionPreparer {
    pub fn new(
        selection: SelectionRecord,
        ready: Value,
        bindings: Vec<ActionBinding>,
        target: (String, String),
        run_directory: PathBuf,
        client: PrivateGuardClient,
        environment: Arc<dyn InstalledSelectedEnvironment>,
    ) -> Result<Self> {
        Self::new_inner(
            selection,
            ready,
            bindings,
            (target.0, target.1, None),
            run_directory,
            client,
            environment,
        )
    }

    pub fn new_matched(
        selection: SelectionRecord,
        ready: Value,
        inspected: &crate::collection_members::InspectedCollectionMembers,
        target: (String, String),
        run_directory: PathBuf,
        client: PrivateGuardClient,
        environment: Arc<dyn InstalledSelectedEnvironment>,
    ) -> Result<Self> {
        ensure!(
            crate::collection_members::is_matched_profile(&inspected.member_profile),
            "matched installed member profile required"
        );
        ensure!(
            inspected.bindings.len()
                == if inspected.expected_snapshot.is_empty() {
                    0
                } else {
                    4
                },
            "matched complete action map unavailable"
        );
        Self::new_inner(
            selection,
            ready,
            inspected.bindings.clone(),
            (
                target.0,
                target.1,
                Some(
                    if inspected.member_profile == crate::collection_members::WORKLOAD_600_PROFILE {
                        crate::collection_members::WORKLOAD_600_NATIVE_PROFILE
                    } else {
                        crate::collection_members::MATCHED_NATIVE_PROFILE
                    },
                ),
            ),
            run_directory,
            client,
            environment,
        )?
        .with_inspected_snapshot(inspected)
    }

    fn new_inner(
        selection: SelectionRecord,
        ready: Value,
        bindings: Vec<ActionBinding>,
        target: (String, String, Option<&'static str>),
        run_directory: PathBuf,
        client: PrivateGuardClient,
        environment: Arc<dyn InstalledSelectedEnvironment>,
    ) -> Result<Self> {
        let matched = target.2.is_some();
        let original = SelectionRecord::decode(
            &selection.original_bytes,
            &selection.selection_ref,
            &selection.selection.owner,
        )?;
        ensure!(
            serde_json::to_value(&original.selection)?
                == serde_json::to_value(&selection.selection)?,
            "selected installed body differs from original admitted bytes"
        );
        ensure!(
            ready["schema"]
                == if matched {
                    crate::collection_members::MATCHED_READY
                } else {
                    "controlled-collection-selected-ready.v2"
                }
                && ready["nativeProfileId"]
                    == if matched {
                        target.2.context("matched native profile unavailable")?
                    } else {
                        "controlled-collection-native-selected.v2-proposal"
                    },
            "selected RPA ready version unavailable"
        );
        ensure!(
            ready["invocationKey"] == selection.selection.invocation_key
                && ready["selectionRawSha256"] == raw_sha256(&selection.original_bytes)
                && ready["selectionRef"] == serde_json::to_value(&selection.selection_ref)?
                && ready["owner"] == serde_json::to_value(&selection.selection.owner)?
                && ready["selection"] == serde_json::to_value(&selection.selection)?,
            "selected native ready association mismatch"
        );
        ensure!(
            ready["navigationSupported"] == matched
                && run_directory.is_absolute()
                && !target.0.is_empty()
                && target.1 == "act",
            "selected installation boundary unavailable"
        );
        ensure!(
            bindings.len() <= 9
                && (!bindings.is_empty()
                    || selection.selection.items.len()
                        == selection.selection.continuation.prefix_length as usize),
            "selected action mapping unavailable"
        );
        let mut nodes = std::collections::BTreeSet::new();
        for binding in &bindings {
            trollm_mcp::policy::validate_task_result_artifact_name(&binding.result_artifact)?;
            ensure!(
                !binding.plan_pin.is_empty()
                    && !binding.node.is_empty()
                    && !binding.task_ref.is_empty()
                    && nodes.insert(binding.node.clone())
                    && (["candidate-select", "quantity-editor", "cart-add"]
                        .contains(&binding.purpose.as_str())
                        || matched && binding.purpose == "navigate-next"),
                "selected action mapping invalid"
            );
        }
        let policy = ready["preparationPolicy"]
            .as_object()
            .context("selected preparation policy unavailable")?;
        ensure!(
            policy.len() == 5
                && [
                    "maxObservationAgeMs",
                    "minPreparationRemainingMs",
                    "selectionExpiresAt",
                    "authorityExpiresAt",
                    "clockPolicy"
                ]
                .iter()
                .all(|key| policy.contains_key(*key)),
            "selected preparation policy is not closed"
        );
        let clock = ready["preparationPolicy"]["clockPolicy"]
            .as_object()
            .context("selected clock policy unavailable")?;
        ensure!(
            clock.len() == 3
                && ["basis", "maxClockErrorMs", "assuranceRef"]
                    .iter()
                    .all(|key| clock.contains_key(*key))
                && clock["basis"] == "bounded-utc"
                && clock["maxClockErrorMs"]
                    .as_u64()
                    .is_some_and(|n| (1..=1000).contains(&n)),
            "selected bounded clock policy unavailable"
        );
        let assurance: crate::collection::OwnedRef =
            serde_json::from_value(clock["assuranceRef"].clone())?;
        ensure!(
            assurance.tenant_id == selection.selection.owner.tenant_id
                && assurance
                    .artifact_id
                    .strip_prefix("art_")
                    .is_some_and(|id| id.len() == 32
                        && id
                            .bytes()
                            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()))
                && assurance.sha256.len() == 64
                && assurance
                    .sha256
                    .bytes()
                    .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()),
            "selected clock assurance owner/hash mismatch"
        );
        let original_cutoffs = [
            "originalCaseExpiresAt",
            "selectionExpiresAt",
            "authorityExpiresAt",
        ];
        let mut minimum = u64::MAX;
        for (index, key) in original_cutoffs.iter().enumerate() {
            let value = if index == 0 {
                &ready[*key]
            } else {
                &ready["preparationPolicy"][*key]
            };
            minimum = minimum.min(parse_selected_utc_ms(
                value.as_str().context("original cutoff unavailable")?,
            )?);
        }
        ensure!(
            parse_selected_utc_ms(
                ready["expiresAt"]
                    .as_str()
                    .context("selected actor cutoff unavailable")?
            )? == minimum,
            "selected actor cutoff does not preserve original expiry minimum"
        );
        Ok(Self {
            selection,
            ready,
            bindings,
            expected_snapshot: None,
            inspected_root_pin: None,
            server: target.0,
            tool: target.1,
            run_directory,
            client,
            environment,
            actor_cutoff_ms: minimum,
            expected_static_key_sha256: None,
            private_installation_key: None,
            intents: std::sync::Mutex::new(std::collections::BTreeMap::new()),
        })
    }
    fn evidence_provider(&self) -> &'static str {
        if self.ready["nativeProfileId"] == crate::collection_members::WORKLOAD_600_NATIVE_PROFILE {
            "rpa-installed-pending-selected-v4"
        } else if self.ready["schema"] == crate::collection_members::MATCHED_READY {
            "rpa-installed-pending-selected-v3"
        } else {
            "rpa-2dadbf-installed-selected-v2"
        }
    }
    /// Native cooperative cancellation, also at model/control boundaries. Clock
    /// loss refuses; a host offset or newly allocated timeout cannot extend it.
    pub(crate) fn start_clock_maintenance(&self) -> Result<()> {
        self.environment.start_clock_maintenance()
    }
    pub(crate) fn stop_clock_maintenance(&self) {
        self.environment.stop_clock_maintenance();
    }
    pub(crate) fn cancellation_cause(&self) -> Option<&'static str> {
        match self.environment.current_utc_upper_ms() {
            Ok(now) if now >= self.actor_cutoff_ms => Some("SELECTED_ACTOR_EXPIRED"),
            Ok(_) => None,
            Err(error)
                if error
                    .downcast_ref::<crate::collection_clock::ClockReadBusy>()
                    .is_some() =>
            {
                None
            }
            Err(error) => Some(
                error
                    .downcast_ref::<crate::collection_clock::ClockFailure>()
                    .map_or("SELECTED_CLOCK_UNAVAILABLE", |failure| failure.0),
            ),
        }
    }
    pub(crate) fn private_invocation_binding(&self) -> Result<(&str, &str)> {
        Ok((
            self.private_installation_key
                .as_deref()
                .context("selected private installation not attached to worker")?,
            self.selection.selection.invocation_key.as_str(),
        ))
    }
    pub(crate) fn installed_key_sha256(&self) -> Result<&str> {
        self.expected_static_key_sha256
            .as_deref()
            .context("installed private credential binding unavailable")
    }
    fn check_retained_intent(
        &self,
        call: &ToolCall,
        context: &PreparationContext,
        evidence: &PreparationEvidence,
    ) -> Result<()> {
        let association = self
            .intents
            .lock()
            .map_err(|_| anyhow::anyhow!("native intent association lock failed"))?
            .get(&context.raw()?.sha256)
            .cloned()
            .context("selected native intent not associated")?;
        let raw = trollm_core::events::retained_prepared_intent(
            &self.run_directory.join("events.jsonl"),
            context,
            call,
            evidence,
            association.0,
        )?;
        ensure!(
            association.1 == raw_sha256(raw.0.as_bytes())
                && association.2 == raw_sha256(raw.1.as_bytes()),
            "original selected native intent/preparation bytes changed"
        );
        Ok(())
    }
    fn current(&self, context: &PreparationContext) -> Result<u64> {
        ensure!(
            self.run_directory.file_name().and_then(|v| v.to_str())
                == Some(context.run_id.as_str()),
            "selected native run association mismatch"
        );
        self.environment
            .check_current_authority(&self.selection, context)?;
        let now = self.environment.current_utc_upper_ms()?;
        let policy = &self.ready["preparationPolicy"];
        for expiry in [
            self.ready["originalCaseExpiresAt"].as_str(),
            policy["selectionExpiresAt"].as_str(),
            policy["authorityExpiresAt"].as_str(),
        ] {
            ensure!(
                now < parse_selected_utc_ms(
                    expiry.context("selected original expiry unavailable")?
                )?,
                "selected authority expired"
            );
        }
        Ok(now)
    }
    /// Native installer supplies only the snapshot rederived by actual member
    /// inspection, never a producer-provided native id or a model argument.
    pub fn with_inspected_snapshot(
        mut self,
        inspected: &crate::collection_members::InspectedCollectionMembers,
    ) -> Result<Self> {
        let prefix = self.selection.selection.continuation.prefix_length as usize;
        let items = &self.selection.selection.items[prefix..];
        ensure!(
            inspected.expected_snapshot.len() == items.len(),
            "selected suffix snapshot length mismatch"
        );
        for (record, item) in inspected.expected_snapshot.iter().zip(items) {
            ensure!(
                record["value"] == serde_json::to_value(item)?,
                "selected suffix full value mismatch"
            );
        }
        ensure!(
            self.bindings.len() == inspected.bindings.len()
                && self
                    .bindings
                    .iter()
                    .zip(&inspected.bindings)
                    .all(|(a, b)| a.plan_pin == b.plan_pin
                        && a.node == b.node
                        && a.task_ref == b.task_ref
                        && a.purpose == b.purpose
                        && a.cursor_node == b.cursor_node
                        && a.result_artifact == b.result_artifact),
            "selected native member binding drift"
        );
        self.expected_snapshot = Some(inspected.expected_snapshot.clone());
        self.inspected_root_pin = Some(inspected.root_plan_pin.clone());
        Ok(self)
    }
    fn check_snapshot(
        &self,
        cursor: &trollm_mcp::preparation::CursorPosition,
        context: &PreparationContext,
    ) -> Result<()> {
        let expected = self
            .expected_snapshot
            .as_ref()
            .context("selected inspected snapshot association unavailable")?;
        let bytes = trollm_core::artifacts::foreach_snapshot_bytes(expected);
        ensure!(
            cursor.snapshot_ref
                == format!(
                    "artifacts/{}",
                    trollm_core::artifacts::foreach_snapshot_name(&cursor.node, &bytes)
                ),
            "selected snapshot reference drift"
        );
        let logical = self.run_directory.join(&cursor.snapshot_ref);
        let path = logical.canonicalize()?;
        ensure!(
            path.starts_with(self.run_directory.canonicalize()?)
                && std::fs::symlink_metadata(&logical)?.is_file(),
            "selected snapshot escapes native custody"
        );
        let actual =
            crate::setup::read_input_evidence(&self.run_directory, &cursor.snapshot_ref, 65536)?;
        ensure!(actual == bytes, "selected full suffix snapshot drift");
        let hash = raw_sha256(&actual);
        let mut committed = None;
        let mut conflicting = false;
        trollm_core::EventLog::visit(
            &self.run_directory.join("events.jsonl"),
            Some(&context.run_id),
            |event, _| {
                if event["type"] == "artifact_written" && event["path"].as_str() == logical.to_str()
                {
                    let next = event["sha256"]
                        .as_str()
                        .zip(event["bytes"].as_u64())
                        .map(|(hash, bytes)| (hash.to_owned(), bytes));
                    if next.is_none()
                        || committed
                            .as_ref()
                            .is_some_and(|previous| Some(previous) != next.as_ref())
                    {
                        conflicting = true;
                    }
                    committed = next;
                }
            },
        )?;
        ensure!(
            !conflicting && committed == Some((hash, actual.len() as u64)),
            "selected snapshot has no actual native artifact record"
        );
        Ok(())
    }
    fn item_request(&self, call: &ToolCall, context: &PreparationContext) -> Result<Value> {
        ensure!(
            call.server == self.server && call.tool == self.tool,
            "selected tool grant mismatch"
        );
        if let Some(root) = &self.inspected_root_pin {
            ensure!(
                context
                    .frames
                    .first()
                    .is_some_and(|frame| &frame.plan == root),
                "selected native root plan drift"
            );
        }
        let binding = self
            .bindings
            .iter()
            .find(|binding| {
                binding.node == context.node
                    && binding.task_ref == context.task_ref
                    && context
                        .frames
                        .last()
                        .is_some_and(|frame| frame.plan == binding.plan_pin)
            })
            .context("unmapped selected action refuses")?;
        let seeded = self.selection.selection.continuation.prefix_length as usize;
        let navigation = binding.purpose == "navigate-next";
        ensure!(
            !navigation || self.ready["navigationSupported"] == true,
            "matched navigation installation unavailable"
        );
        let index = match &binding.cursor_node {
            Some(node) => {
                let cursors: Vec<_> = context
                    .cursors
                    .iter()
                    .filter(|cursor| &cursor.node == node)
                    .collect();
                ensure!(
                    cursors.len() == 1
                        && cursors[0].phase == if navigation { "inBetween" } else { "inBody" },
                    "selected native cursor unavailable"
                );
                self.check_snapshot(cursors[0], context)?;
                seeded
                    .checked_add(usize::try_from(cursors[0].index)?)
                    .context("selected cursor overflow")?
            }
            None => {
                ensure!(
                    self.selection.selection.items.len().saturating_sub(seeded) == 1,
                    "selected uncursored action requires exactly one admitted suffix item"
                );
                seeded
            }
        };
        ensure!(
            !navigation
                || index
                    .checked_add(1)
                    .is_some_and(|next| next < self.selection.selection.items.len()),
            "navigation after final admitted item refuses before preparation"
        );
        let item = self
            .selection
            .selection
            .items
            .get(index)
            .context("no selected effect admitted")?;
        let action = if navigation {
            "navigate"
        } else if binding.purpose == "quantity-editor" {
            "type"
        } else {
            "click"
        };
        ensure!(
            call.args["action"] == action,
            "selected proposal action mismatch"
        );
        if action == "type" {
            ensure!(
                call.args["value"] == item.quantity_text,
                "selected proposal quantity mismatch"
            );
        }
        let raw = context.raw()?;
        ensure!(
            raw.json.len() <= 16384,
            "selected caller context exceeds wire bound"
        );
        let (model_slot, model_call_ordinal) = context
            .origin
            .model_identity()
            .context("selected preparation requires model execution")?;
        let association = serde_json::to_vec(&json!({"runId":context.run_id,"node":context.node,
            "taskRef":context.task_ref,"frames":context.frames,"cursors":context.cursors,
            "modelSlot":model_slot,"modelCallOrdinal":model_call_ordinal,
            "originalItemId":item.original_item_id,"admittedOrder":item.admitted_order}))?;
        ensure!(
            association.len() <= 16384,
            "selected caller association exceeds wire bound"
        );
        Ok(
            json!({"kind":"prepare","selectionRef":self.selection.selection_ref,
            "selectionRawSha256":raw_sha256(&self.selection.original_bytes),"owner":self.selection.selection.owner,
            "selectionId":self.selection.selection.selection_id,"caseId":self.selection.selection.case_id,
            "invocationKey":self.selection.selection.invocation_key,"originalItemId":item.original_item_id,
            "inputPosition":item.input_position,"admittedOrder":item.admitted_order,"quantity":item.quantity,
            "quantityText":item.quantity_text,"purpose":binding.purpose,
            "callerContext":{"bytesBase64":base64::engine::general_purpose::STANDARD.encode(raw.json.as_bytes()),"sha256":raw.sha256},
            "callerAssociation":{"bytesBase64":base64::engine::general_purpose::STANDARD.encode(&association),"sha256":raw_sha256(&association)}}),
        )
    }
    fn selected_page_url(&self, index: u64) -> Result<String> {
        let mut url = url::Url::parse(
            self.ready["currentUrl"]
                .as_str()
                .context("selected original fixture URL unavailable")?,
        )?;
        let first = u64::from(self.selection.selection.continuation.prefix_length) + 1;
        ensure!(
            url.scheme() == "http"
                && url.host_str() == Some("127.0.0.1")
                && url.username().is_empty()
                && url.password().is_none()
                && url.query().is_none()
                && url.fragment().is_none()
                && url.path() == format!("/collection/{first}"),
            "selected original fixture page/origin mismatch"
        );
        url.set_path(&format!(
            "/collection/{}",
            index
                .checked_add(1)
                .context("selected page index overflow")?
        ));
        Ok(url.into())
    }
    fn check_guard(
        &self,
        guard: &Value,
        request: &Value,
        context: &PreparationContext,
        completion: bool,
    ) -> Result<()> {
        let now = self.current(context)?;
        ensure!(
            guard["schema"]
                == if self.ready["schema"] == crate::collection_members::MATCHED_READY {
                    crate::collection_members::MATCHED_GUARD
                } else {
                    "controlled-collection-selected-guard.v2"
                }
                && guard["invocationKey"] == self.selection.selection.invocation_key
                && guard["selectionRef"] == request["selectionRef"]
                && guard["owner"] == request["owner"]
                && guard["selectionRawSha256"] == request["selectionRawSha256"]
                && guard["selectionId"] == request["selectionId"]
                && guard["caseId"] == request["caseId"]
                && guard["callerContext"] == request["callerContext"]
                && guard["callerAssociation"] == request["callerAssociation"]
                && guard["purpose"] == request["purpose"],
            "selected guard association mismatch"
        );
        for (key, field) in [
            ("originalItemId", "originalItemId"),
            ("inputPosition", "inputPosition"),
            ("admittedOrder", "admittedOrder"),
            ("quantity", "quantity"),
            ("quantityText", "quantityText"),
        ] {
            ensure!(
                guard["item"][key] == request[field],
                "selected guard item mismatch"
            );
        }
        let navigation = request["purpose"] == "navigate-next";
        let order = request["admittedOrder"]
            .as_u64()
            .context("selected order unavailable")?;
        let current_url = if navigation || self.ready["navigationSupported"] == true {
            self.selected_page_url(order)?
        } else {
            self.ready["currentUrl"]
                .as_str()
                .context("selected initial URL missing")?
                .to_owned()
        };
        ensure!(
            guard["browser"] == self.ready["browser"]
                && guard["tabId"] == self.ready["tabId"]
                && guard["currentUrl"] == current_url
                && guard["sifrSessionId"]
                    .as_str()
                    .is_some_and(|s| !s.is_empty()),
            "selected guard target association unavailable"
        );
        let known = guard["knownCommittedPrefixCount"]
            .as_u64()
            .context("selected committed prefix missing")?;
        let seeded = u64::from(self.selection.selection.continuation.prefix_length);
        ensure!(
            guard["seededPrefixCount"] == seeded
                && known
                    == order
                        .checked_add(u64::from(navigation))
                        .context("selected prefix overflow")?
                && known >= seeded
                && guard["newCommittedAdds"] == known - seeded,
            "selected prefix/new-effect association mismatch"
        );
        let args = &guard["args"];
        ensure!(
            args["browser"] == guard["browser"]
                && args["sessionId"] == guard["sifrSessionId"]
                && args["target"].as_str().is_some_and(|s| !s.is_empty())
                && guard["selector"].as_str().is_some_and(|s| !s.is_empty())
                && guard["nodeId"].as_str().is_some_and(|s| !s.is_empty()),
            "selected final arguments association mismatch"
        );
        if navigation {
            ensure!(
                guard["fixturePurpose"] == "catalog-link",
                "navigation fixture purpose is not the native operation purpose"
            );
            ensure!(
                args["target"]
                    == self.selected_page_url(
                        order
                            .checked_add(1)
                            .context("selected next-page overflow")?
                    )?
                    && args["target"] != guard["selector"],
                "navigation URL is not the captured DOM selector or an arbitrary next page"
            );
        } else {
            ensure!(
                args["target"] == guard["selector"],
                "selected target selector mismatch"
            );
        }
        let fields = args
            .as_object()
            .context("selected arguments object missing")?;
        let expected = if request["purpose"] == "quantity-editor" {
            vec!["action", "browser", "sessionId", "target", "value"]
        } else {
            vec!["action", "browser", "sessionId", "target"]
        };
        ensure!(
            fields.len() == expected.len() && expected.iter().all(|key| fields.contains_key(*key)),
            "selected arguments contain an unmapped field"
        );
        let quantity = request["purpose"] == "quantity-editor";
        ensure!(
            args["action"]
                == if navigation {
                    "navigate"
                } else if quantity {
                    "type"
                } else {
                    "click"
                },
            "selected guard action mismatch"
        );
        if quantity {
            ensure!(
                args["value"] == request["quantityText"],
                "selected guard quantity mismatch"
            );
        }
        let policy = &self.ready["preparationPolicy"];
        let age = policy["maxObservationAgeMs"]
            .as_u64()
            .filter(|n| *n > 0 && *n <= 120000)
            .context("selected age policy unavailable")?;
        let reserve = policy["minPreparationRemainingMs"]
            .as_u64()
            .filter(|n| *n > 0 && *n <= age)
            .context("selected reserve policy unavailable")?;
        ensure!(
            guard["maxObservationAgeMs"] == age
                && guard["minPreparationRemainingMs"] == reserve
                && guard["clockPolicy"] == policy["clockPolicy"],
            "selected guard policy drift"
        );
        let started = parse_selected_utc_ms(
            guard["observationStartedAt"]
                .as_str()
                .context("observation start missing")?,
        )?;
        let finished = parse_selected_utc_ms(
            guard["observationFinishedAt"]
                .as_str()
                .context("observation finish missing")?,
        )?;
        let expiry = parse_selected_utc_ms(
            guard["effectiveExpiresAt"]
                .as_str()
                .context("effective expiry missing")?,
        )?;
        ensure!(
            started <= finished && finished <= now && now - started <= age && now < expiry,
            "selected observation stale or expired"
        );
        let clock = &guard["clockVerification"];
        let clock_fields = clock
            .as_object()
            .context("selected measured clock coverage unavailable")?;
        ensure!(
            clock_fields.len() == 6
                && clock["verified"] == true
                && clock["assuranceRef"] == policy["clockPolicy"]["assuranceRef"]
                && clock["rawSha256"] == policy["clockPolicy"]["assuranceRef"]["sha256"]
                && clock["maxClockErrorMs"] == policy["clockPolicy"]["maxClockErrorMs"],
            "selected measured clock association mismatch"
        );
        let error = clock["maxClockErrorMs"]
            .as_u64()
            .context("selected clock bound missing")?;
        ensure!(
            parse_selected_utc_ms(
                clock["validFrom"]
                    .as_str()
                    .context("clock coverage start missing")?
            )? <= started
                && parse_selected_utc_ms(
                    clock["validUntil"]
                        .as_str()
                        .context("clock coverage end missing")?
                )? >= finished,
            "selected acquisitions outside measured clock coverage"
        );
        let intervals = guard["observationAcquisitions"]
            .as_array()
            .context("selected acquisition intervals missing")?;
        ensure!(
            !intervals.is_empty() && intervals.len() <= 64,
            "selected acquisition intervals unbounded"
        );
        let mut oldest = u64::MAX;
        for interval in intervals {
            let began = parse_selected_utc_ms(
                interval["startedAt"]
                    .as_str()
                    .context("acquisition start missing")?,
            )?;
            let ended = parse_selected_utc_ms(
                interval["finishedAt"]
                    .as_str()
                    .context("acquisition end missing")?,
            )?;
            let elapsed = interval["elapsedMonotonicMs"]
                .as_f64()
                .filter(|n| n.is_finite() && *n >= 0.0)
                .context("acquisition elapsed unavailable")?;
            ensure!(
                began <= ended
                    && ended <= now.saturating_add(error)
                    && ((ended - began) as f64 - elapsed).abs() <= (2 * error + 1) as f64,
                "selected acquisition discontinuity"
            );
            oldest = oldest.min(began);
        }
        ensure!(
            oldest.checked_sub(error) == Some(started),
            "selected oldest observation was restamped"
        );
        let installer = &guard["installerClock"];
        ensure!(
            installer["source"] == "RPA38c-kernel-clock-assurance"
                && installer["invalid"] == false
                && installer["units"] == "milliseconds-since-unix-epoch"
                && installer["clockDomain"]["kind"] == "linux-CLOCK_REALTIME"
                && installer["lastSample"]["kernelAssured"] == true
                && installer["lastSample"]["finishedAt"] == clock["validUntil"],
            "selected installed clock provenance unavailable"
        );
        let original = &guard["originalExpiryEvidence"];
        ensure!(
            original["caseExpiresAt"] == self.ready["originalCaseExpiresAt"]
                && original["selectionExpiresAt"] == policy["selectionExpiresAt"]
                && original["authorityExpiresAt"] == policy["authorityExpiresAt"]
                && original["supportRef"] == self.ready["policyEvidence"]["ref"]
                && original["supportRawSha256"] == self.ready["policyEvidence"]["rawSha256"]
                && original["supplied"] == self.ready["expiryEvidence"],
            "selected original cutoff drift"
        );
        let maximum = [
            parse_selected_utc_ms(original["caseExpiresAt"].as_str().unwrap())?,
            parse_selected_utc_ms(original["selectionExpiresAt"].as_str().unwrap())?,
            parse_selected_utc_ms(original["authorityExpiresAt"].as_str().unwrap())?,
            started
                .checked_add(age)
                .context("selected observation expiry overflow")?,
        ]
        .into_iter()
        .min()
        .unwrap();
        ensure!(expiry <= maximum, "selected effective cutoff extended");
        if completion {
            ensure!(
                expiry - now >= reserve,
                "selected preparation reserve insufficient"
            );
        }
        Ok(())
    }
}
impl CallPreparer for InstalledCollectionPreparer {
    fn prepared_argument_names(&self, server: &str, tool: &str) -> Vec<String> {
        if server == self.server && tool == self.tool {
            ["browser", "sessionId", "target"]
                .into_iter()
                .map(String::from)
                .collect()
        } else {
            Vec::new()
        }
    }

    fn after_retained_intent(
        &self,
        call: &ToolCall,
        context: &PreparationContext,
        evidence: &PreparationEvidence,
        intent_seq: u64,
    ) -> Result<()> {
        self.current(context)?;
        let raw = trollm_core::events::retained_prepared_intent(
            &self.run_directory.join("events.jsonl"),
            context,
            call,
            evidence,
            intent_seq,
        )?;
        let association = (
            intent_seq,
            raw_sha256(raw.0.as_bytes()),
            raw_sha256(raw.1.as_bytes()),
        );
        let mut retained = self
            .intents
            .lock()
            .map_err(|_| anyhow::anyhow!("native intent association lock failed"))?;
        let key = context.raw()?.sha256;
        if let Some(previous) = retained.get(&key) {
            ensure!(
                *previous == association,
                "native intent association conflict"
            );
        } else {
            retained.insert(key, association);
        }
        Ok(())
    }
    fn prepare(
        &self,
        call: &ToolCall,
        context: &PreparationContext,
        deadline: Option<Instant>,
    ) -> Result<PreparedCall> {
        ensure!(
            context.origin.model_identity().is_some(),
            "selected preparation requires model execution"
        );
        self.current(context)?;
        let request = self.item_request(call, context)?;
        let reference = serde_json::to_value(&self.selection.selection_ref)?;
        let wrapper = GuardRequest::new(
            &self.selection.selection.invocation_key,
            &reference,
            context,
            &request,
        )?;
        let deadline = deadline.context("selected native deadline required")?;
        let response = self
            .client
            .prepare(&wrapper, deadline, &|| self.current(context).map(|_| ()))?;
        let guard: Value = serde_json::from_slice(&response.bytes)?;
        self.check_guard(&guard, &request, context, true)?;
        Ok(PreparedCall {
            call: ToolCall {
                server: call.server.clone(),
                tool: call.tool.clone(),
                args: guard["args"].clone(),
            },
            evidence: PreparationEvidence {
                provider: self.evidence_provider().into(),
                context_sha256: context.raw()?.sha256,
                response_json: String::from_utf8(response.bytes)?,
                response_sha256: response.sha256,
            },
        })
    }
    fn acquire_before_send(
        &self,
        resolved: &ToolCall,
        context: &PreparationContext,
        evidence: &PreparationEvidence,
    ) -> Result<()> {
        ensure!(
            evidence.provider == self.evidence_provider()
                && evidence.context_sha256 == context.raw()?.sha256
                && evidence.response_sha256 == raw_sha256(evidence.response_json.as_bytes()),
            "selected preparation evidence drift"
        );
        self.check_retained_intent(resolved, context, evidence)?;
        let guard: Value = serde_json::from_str(&evidence.response_json)?;
        let request = self.item_request(resolved, context)?;
        self.check_guard(&guard, &request, context, false)?;
        ensure!(
            guard["args"] == resolved.args,
            "selected sent arguments changed after intent"
        );
        // Journal/argument validation above can involve I/O. Read actual current
        // authority/device time again after it, without renewing the cutoff.
        let now = self.current(context)?;
        let cutoff = parse_selected_utc_ms(
            guard["effectiveExpiresAt"]
                .as_str()
                .context("selected cutoff missing")?,
        )?;
        ensure!(
            now < cutoff,
            "selected preparation expired after final validation"
        );
        let remaining = Duration::from_millis((cutoff - now).min(30000));
        let control = self.client.control(Instant::now() + remaining, &|| {
            self.current(context).map(|_| ())
        })?;
        ensure!(
            !control.aborted
                && control.invocation_key == self.selection.selection.invocation_key
                && control.expires_at
                    == self.ready["expiresAt"]
                        .as_str()
                        .context("selected actor cutoff missing")?,
            "selected guard actor aborted or association changed"
        );
        // /control is adapter liveness/abort only, never native worker stop.
        ensure!(
            self.current(context)? < cutoff,
            "selected preparation expired after actor control read"
        );
        Ok(())
    }
    fn check_before_send(
        &self,
        resolved: &ToolCall,
        context: &PreparationContext,
        evidence: &PreparationEvidence,
    ) -> Result<()> {
        ensure!(
            evidence.context_sha256 == context.raw()?.sha256
                && evidence.response_sha256 == raw_sha256(evidence.response_json.as_bytes()),
            "selected local context/evidence drift"
        );
        ensure!(
            self.intents
                .lock()
                .map_err(|_| anyhow::anyhow!("native intent association lock failed"))?
                .contains_key(&evidence.context_sha256),
            "selected local intent association unavailable"
        );
        let guard: Value = serde_json::from_str(&evidence.response_json)?;
        ensure!(
            guard["args"] == resolved.args,
            "selected local final arguments drift"
        );
        let now = self
            .environment
            .local_write_utc_upper_ms(&self.selection, context)?;
        for cutoff in [
            guard["effectiveExpiresAt"].as_str(),
            self.ready["originalCaseExpiresAt"].as_str(),
            self.ready["preparationPolicy"]["selectionExpiresAt"].as_str(),
            self.ready["preparationPolicy"]["authorityExpiresAt"].as_str(),
        ] {
            ensure!(
                now < parse_selected_utc_ms(
                    cutoff.context("selected local original cutoff missing")?
                )?,
                "selected local cutoff expired"
            );
        }
        Ok(())
    }
    fn after_retained_result(
        &self,
        resolved: &ToolCall,
        context: &PreparationContext,
        evidence: &PreparationEvidence,
        result: &ToolResult,
        intent: Option<u64>,
        result_seq: u64,
    ) -> Result<()> {
        ensure!(
            evidence.context_sha256 == context.raw()?.sha256,
            "selected retained context drift"
        );
        ensure!(
            intent.is_some()
                && self
                    .intents
                    .lock()
                    .map_err(|_| anyhow::anyhow!("native intent association lock failed"))?
                    .get(&context.raw()?.sha256)
                    .map(|association| association.0)
                    == intent,
            "selected result does not match associated native intent"
        );
        self.check_retained_intent(resolved, context, evidence)?;
        let binding = self
            .bindings
            .iter()
            .find(|binding| {
                binding.node == context.node
                    && binding.task_ref == context.task_ref
                    && context
                        .frames
                        .last()
                        .is_some_and(|frame| frame.plan == binding.plan_pin)
            })
            .context("selected retained result mapping unavailable")?;
        let outcome = retained_outcome_request(
            &self.selection,
            context,
            resolved,
            result,
            &self.run_directory,
            (intent.context("selected intent unavailable")?, result_seq),
            &binding.result_artifact,
        )?;
        let reference = serde_json::to_value(&self.selection.selection_ref)?;
        let request = GuardRequest::new(
            &self.selection.selection.invocation_key,
            &reference,
            context,
            &outcome,
        )?;
        let bytes =
            trollm_mcp::preparation::js_sorted_json_bytes(&serde_json::to_value(&request)?)?;
        let directory = self.run_directory.join("selected-report");
        std::fs::create_dir_all(&directory)?;
        ensure!(
            std::fs::symlink_metadata(&directory)?.is_dir(),
            "selected report directory is not a native directory"
        );
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))?;
        retain_identical(
            &directory.join(format!("{}.request.json", context.raw()?.sha256)),
            &bytes,
        )?;
        let response = self.client.acknowledge_outcome(
            &request,
            Instant::now() + Duration::from_secs(30),
            &|| self.current(context).map(|_| ()),
        )?;
        retain_identical(
            &directory.join(format!("{}.ack.json", context.raw()?.sha256)),
            &response.bytes,
        )?;
        Ok(())
    }
}
fn retain_identical(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    ensure!(
        bytes.len() <= 262144,
        "selected retained report exceeds bound"
    );
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
    {
        Ok(mut file) => {
            file.write_all(bytes)?;
            file.sync_all()?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            let metadata = std::fs::symlink_metadata(path)?;
            ensure!(
                metadata.is_file()
                    && !metadata.file_type().is_symlink()
                    && metadata.len() == bytes.len() as u64
                    && std::fs::read(path)? == bytes,
                "selected report identity conflict"
            );
        }
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod controls {
    use super::*;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use trollm_mcp::{CallContext, RootCertificates};
    #[allow(dead_code)] // Reuse the complete existing test-only CA fixture unchanged.
    mod tls {
        include!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../trollm-mcp/tests/common/tls_fixture.rs"
        ));
    }
    pub(crate) struct Environment {
        pub(crate) now: AtomicU64,
        revoked: AtomicBool,
    }
    pub(crate) fn actor_for_control() -> (Arc<InstalledCollectionPreparer>, Arc<Environment>) {
        let environment = Arc::new(Environment {
            now: AtomicU64::new(parse_selected_utc_ms("2026-11-04T10:00:00.000Z").unwrap()),
            revoked: AtomicBool::new(false),
        });
        let mut preparer = fixture(
            "https://127.0.0.1:9/guard".into(),
            environment.clone(),
            PathBuf::from("/tmp/controlled-selected-run"),
        );
        // Declared control bytes only, not an installed private credential.
        preparer.private_installation_key = Some("a".repeat(64));
        (Arc::new(preparer), environment)
    }

    pub(crate) fn actor_for_environment(
        environment: Arc<dyn InstalledSelectedEnvironment>,
    ) -> Arc<InstalledCollectionPreparer> {
        let (preparer, _) = actor_for_control();
        let mut preparer = Arc::try_unwrap(preparer).ok().unwrap();
        preparer.environment = environment;
        // Explicit test-only actor bound. Real expiry still comes from original
        // ready/selection; this fixture tests the maintained model wrapper only.
        preparer.actor_cutoff_ms = u64::MAX;
        Arc::new(preparer)
    }

    #[test]
    fn binding_refusals_keep_distinct_codes_original_allocated_id_and_no_worker_proof() {
        struct Provider {
            mode: crate::ConnectionAuth,
            key: Option<String>,
        }
        impl crate::TokenProvider for Provider {
            fn connection_auth(&self, name: String, url: String) -> crate::ConnectionAuth {
                assert_eq!(name, "e2llm");
                assert_eq!(url, "https://selected.invalid/mcp");
                self.mode.clone()
            }
            fn bearer_token(&self, name: String, url: String) -> Option<String> {
                assert_eq!(name, "e2llm");
                assert_eq!(url, "https://selected.invalid/mcp");
                self.key.clone()
            }
        }
        for (mode, key, expected) in [
            (
                crate::ConnectionAuth::Unavailable,
                None,
                "SELECTED_BIND_STATIC_KEY_MODE",
            ),
            (
                crate::ConnectionAuth::Bearer,
                None,
                "SELECTED_BIND_STATIC_KEY_READ",
            ),
            (
                crate::ConnectionAuth::Bearer,
                Some("wrong-original"),
                "SELECTED_BIND_STATIC_KEY_HASH",
            ),
            (
                crate::ConnectionAuth::Bearer,
                Some("declared-original"),
                "SELECTED_BIND_SOURCE_SELECTION",
            ),
        ] {
            let base = std::env::temp_dir().join(format!(
                "selected-refusal-{}-{expected}",
                std::process::id()
            ));
            let run = trollm_core::RunDir::under(&base);
            run.create_dirs().unwrap();
            let client = PrivateGuardClient::new(
                "https://127.0.0.1:9/guard",
                Arc::new(|| Some("declared-unused-guard".into())),
                RootCertificates::from_pem(tls::ROOT_1_PEM).unwrap(),
            )
            .unwrap();
            let installation = SelectedRunInstallation::new(
                json!({}),
                client,
                1,
                raw_sha256(b"declared-original"),
            )
            .unwrap();
            let inspected = crate::collection_members::InspectedCollectionMembers {
                member_profile: "unused".into(),
                root_plan_pin: "unused".into(),
                bindings: vec![],
                expected_snapshot: vec![],
                selection_input_json: "unused".into(),
            };
            let provider: crate::SharedTokenProvider = Arc::new(Box::new(Provider {
                mode,
                key: key.map(str::to_owned),
            }));
            let failure = installation
                .bind(
                    &base,
                    run.path(),
                    &json!({}),
                    &inspected,
                    provider,
                    "https://selected.invalid/mcp",
                )
                .err()
                .unwrap();
            let typed = failure
                .downcast_ref::<crate::selected_start::Failure>()
                .unwrap();
            assert_eq!(typed.code, expected);
            assert_eq!(typed.native_run_id.as_deref(), Some(run.run_id()));
            let record: Value = serde_json::from_slice(
                &std::fs::read(run.path().join("selected-start-failure.json")).unwrap(),
            )
            .unwrap();
            assert_eq!(record["code"], expected);
            assert_eq!(record["workerInactiveAcknowledged"], false);
            assert!(!run.path().join("events.jsonl").exists());
            assert!(!run.path().join("selected-invocation.json").exists());
            std::fs::remove_dir_all(base).unwrap();
        }
    }

    #[test]
    fn full_matched_bind_joins_static_auth_original_members_two_calibrations_and_actor_control() {
        use crate::collection_members::{
            CollectionMembers, CollectionSource, MATCHED_NATIVE_PROFILE, MATCHED_PROFILE,
            MATCHED_READY, SOURCE_SCHEMA,
        };
        use std::sync::atomic::AtomicUsize;
        struct Provider {
            id: String,
            calls: Arc<AtomicUsize>,
        }
        impl crate::TokenProvider for Provider {
            fn bearer_token(&self, name: String, url: String) -> Option<String> {
                assert_eq!(name, "e2llm");
                assert_eq!(url, "https://api.e2llm.dev/mcp");
                Some("declared-static-key".into())
            }
            fn source_authority_current(&self, raw: String) -> bool {
                let value: Value = serde_json::from_str(&raw).unwrap();
                assert_eq!(value["nativeRunId"], self.id);
                self.calls.fetch_add(1, Ordering::SeqCst);
                true
            }
        }
        for workload in [false, true] {
            let mut body = crate::collection::tests::body();
            let context = b"declared full binding context";
            body["contextRef"]["sha256"] = json!(raw_sha256(context));
            let selection = crate::collection::tests::decode(&body).unwrap();
            let members = if workload {
                CollectionMembers::compile_matched_600(
                    &selection,
                    context,
                    "https://api.e2llm.dev/mcp",
                )
            } else {
                CollectionMembers::compile_matched(&selection, context, "https://api.e2llm.dev/mcp")
            }
            .unwrap();
            let installation = raw_sha256(&members.files["manifest.json"]);
            let root = std::env::temp_dir().join(format!(
                "selected-positive-bind-{}",
                trollm_core::RunDir::under(Path::new("unused")).run_id()
            ));
            std::fs::create_dir_all(root.join("bundle-revisions")).unwrap();
            let directory = root.join("bundle-revisions").join(&installation);
            members.write_new(&directory).unwrap();
            let source = CollectionSource {
                schema_version: SOURCE_SCHEMA.into(),
                origin: "DECLARED".into(),
                owner: selection.selection.owner.clone(),
                fixture_id: "declared-fixture".into(),
                version_id: "declared-version".into(),
                selection_ref: selection.selection_ref.clone(),
                selection_raw_sha256: raw_sha256(&selection.original_bytes),
                profile_id: if workload {
                    crate::collection_members::WORKLOAD_600_PROFILE
                } else {
                    MATCHED_PROFILE
                }
                .into(),
                profile_sha256: raw_sha256(&members.files["collection-profile.json"]),
                installation_sha256: installation.clone(),
                qualification: "unqualified".into(),
            };
            let inspected = source
                .inspect(&directory, "https://api.e2llm.dev/mcp")
                .unwrap();
            let (client, policy, assurance, server, requests, clock_directory) =
                crate::collection_clock::controls::binding_fixture(
                    crate::collection::tests::decode(&body).unwrap(),
                );
            let expiry = "2099-01-01T00:00:00.000Z";
            let ready = json!({"schema":MATCHED_READY,"nativeProfileId":if workload { crate::collection_members::WORKLOAD_600_NATIVE_PROFILE } else { MATCHED_NATIVE_PROFILE },
            "invocationKey":body["invocationKey"],"selectionRawSha256":raw_sha256(&selection.original_bytes),
            "selectionRef":selection.selection_ref,"owner":body["owner"],"selection":body,"navigationSupported":true,
            "expiresAt":expiry,"originalCaseExpiresAt":expiry,"preparationPolicy":{
                "maxObservationAgeMs":1000,"minPreparationRemainingMs":100,"selectionExpiresAt":expiry,"authorityExpiresAt":expiry,
                "clockPolicy":{"basis":"bounded-utc","maxClockErrorMs":1000,"assuranceRef":assurance}}});
            let run = trollm_core::RunDir::under(&root);
            run.create_dirs().unwrap();
            let association = json!({"nativeRunId":run.run_id(),"installation":{"installationSha256":installation},
            "descriptor":{"source":source,"expiresAt":expiry,"identity":{"manifestSha256":installation}}});
            let calls = Arc::new(AtomicUsize::new(0));
            let provider: crate::SharedTokenProvider = Arc::new(Box::new(Provider {
                id: run.run_id().to_owned(),
                calls: calls.clone(),
            }));
            let installed = SelectedRunInstallation::new_calibrated(
                ready,
                client,
                policy,
                raw_sha256(b"declared-static-key"),
            )
            .unwrap()
            .with_private_installation_key("a".repeat(64));
            let preparer = installed
                .bind(
                    &root,
                    run.path(),
                    &association,
                    &inspected,
                    provider,
                    "https://api.e2llm.dev/mcp",
                )
                .unwrap();
            server.join().unwrap();
            assert_eq!(requests.load(Ordering::SeqCst), 3);
            assert_eq!(calls.load(Ordering::SeqCst), 2);
            assert_eq!(
                std::fs::read_dir(run.path())
                    .unwrap()
                    .filter(|e| e
                        .as_ref()
                        .unwrap()
                        .file_name()
                        .to_string_lossy()
                        .starts_with("clock-calibration-"))
                    .count(),
                14
            );
            assert!(run
                .path()
                .join("selected-private-installation.json")
                .exists());
            assert!(run.path().join("selected-invocation.json").exists());
            assert!(!run.path().join("events.jsonl").exists()); // Binding alone starts no worker/model.
            drop(preparer);
            std::fs::remove_dir_all(root).unwrap();
            std::fs::remove_dir_all(clock_directory).unwrap();
        }
    }

    #[test]
    fn selected_runtime_configuration_cannot_redirect_or_replace_model() {
        let original = json!({"version":1,"models":{"coder":{"path":"/app/coder.gguf",
            "sha256":"bd258782e35f7f458f8aced1adc053e6e92e89bc735ba3be89d38a06121dc517",
            "ctx":4096,"temperature":0.2,"topP":0.95}},
            "connections":{"e2llm":{"url":"https://api.e2llm.dev/mcp"}}});
        for (pointer, changed) in [
            ("/models/coder/ctx", json!(8192)),
            ("/models/coder/sha256", json!("0".repeat(64))),
            ("/models/coder/temperature", json!(0.0)),
            ("/connections/e2llm/url", json!("https://mcp.e2llm.dev/mcp")),
        ] {
            let mut value = original.clone();
            *value.pointer_mut(pointer).unwrap() = changed;
            assert!(SelectedRunInstallation::validate_configuration(
                &serde_json::from_value(value).unwrap(),
                "https://api.e2llm.dev/mcp"
            )
            .is_err());
        }
        assert!(SelectedRunInstallation::validate_configuration(
            &serde_json::from_value(original).unwrap(),
            "https://api.e2llm.dev/mcp"
        )
        .is_ok());
    }

    #[test]
    fn native_invocation_claim_refuses_fresh_duplicate_changed_body_and_partial_index() {
        let base = std::env::temp_dir().join(format!(
            "selected-claim-{}",
            trollm_core::RunDir::under(Path::new("unused")).run_id()
        ));
        let first = trollm_core::RunDir::under(&base);
        first.create_dirs().unwrap();
        let second = trollm_core::RunDir::under(&base);
        second.create_dirs().unwrap();
        let selection =
            crate::collection::tests::decode(&crate::collection::tests::body()).unwrap();
        let association = json!({"nativeRunId":first.run_id(),"installation":{"installationSha256":"0".repeat(64)}});
        retain_selected_run_claim(&base, first.path(), &association, &selection).unwrap();
        let original = std::fs::read(first.path().join("selected-invocation.json")).unwrap();
        assert!(retain_selected_run_claim(&base, first.path(), &association, &selection).is_err());
        let mut different = association.clone();
        different["nativeRunId"] = json!(second.run_id());
        assert!(retain_selected_run_claim(&base, second.path(), &different, &selection).is_err());
        let mut body = crate::collection::tests::body();
        body["items"][0]["quantity"] = json!(3);
        body["items"][0]["quantityText"] = json!("3");
        let changed = crate::collection::tests::decode(&body).unwrap();
        assert!(retain_selected_run_claim(&base, second.path(), &different, &changed).is_err());
        assert_eq!(
            std::fs::read(first.path().join("selected-invocation.json")).unwrap(),
            original
        );
        let entry = std::fs::read_dir(base.join("selected-invocations"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        std::fs::write(&entry, b"partial-test-claim").unwrap();
        assert!(retain_selected_run_claim(&base, second.path(), &different, &selection).is_err());
        assert_eq!(std::fs::read(entry).unwrap(), b"partial-test-claim");
        std::fs::remove_dir_all(base).unwrap();
    }
    impl InstalledSelectedEnvironment for Environment {
        fn check_current_authority(
            &self,
            _: &SelectionRecord,
            _: &PreparationContext,
        ) -> Result<()> {
            ensure!(
                !self.revoked.load(Ordering::SeqCst),
                "controlled revoked native authority"
            );
            Ok(())
        }
        fn current_utc_upper_ms(&self) -> Result<u64> {
            Ok(self.now.load(Ordering::SeqCst))
        }
        fn local_write_utc_upper_ms(
            &self,
            selection: &SelectionRecord,
            context: &PreparationContext,
        ) -> Result<u64> {
            self.check_current_authority(selection, context)?;
            self.current_utc_upper_ms()
        }
    }
    #[test]
    fn acquisition_only_environment_cannot_supply_final_local_authority() {
        struct AcquisitionOnly;
        impl InstalledSelectedEnvironment for AcquisitionOnly {
            fn check_current_authority(
                &self,
                _: &SelectionRecord,
                _: &PreparationContext,
            ) -> Result<()> {
                Ok(())
            }
            fn current_utc_upper_ms(&self) -> Result<u64> {
                Ok(0)
            }
        }
        let selection =
            crate::collection::tests::decode(&crate::collection::tests::body()).unwrap();
        assert!(AcquisitionOnly
            .local_write_utc_upper_ms(&selection, &context())
            .is_err());
    }
    fn fixture(
        url: String,
        now: Arc<Environment>,
        directory: PathBuf,
    ) -> InstalledCollectionPreparer {
        let mut body = crate::collection::tests::body();
        let item = body["items"][0].clone();
        body["items"] = json!([item]);
        let selection = crate::collection::tests::decode(&body).unwrap();
        let ready = json!({"schema":"controlled-collection-selected-ready.v2","nativeProfileId":"controlled-collection-native-selected.v2-proposal",
            "invocationKey":body["invocationKey"],"selectionRawSha256":raw_sha256(&selection.original_bytes),
            "selectionRef":selection.selection_ref,"owner":body["owner"],"selection":body,
            "navigationSupported":false,"originalCaseExpiresAt":"2026-11-04T10:00:10.000Z", "expiresAt":"2026-11-04T10:00:08.000Z",
            "currentUrl":"https://fixture.invalid/catalog","browser":"fixture-browser","tabId":3,
            "policyEvidence":{"ref":body["supportRef"],"rawSha256":body["supportRef"]["sha256"]},
            "expiryEvidence":{"controlledProjection":true},
            "preparationPolicy":{"maxObservationAgeMs":1000,"minPreparationRemainingMs":100,
                "selectionExpiresAt":"2026-11-04T10:00:08.000Z","authorityExpiresAt":"2026-11-04T10:00:09.000Z",
                "clockPolicy":{"basis":"bounded-utc","maxClockErrorMs":1,"assuranceRef":selection.selection_ref}}});
        let client = PrivateGuardClient::new(
            &url,
            Arc::new(|| Some("controlled-private-token".into())),
            RootCertificates::from_pem(tls::ROOT_1_PEM).unwrap(),
        )
        .unwrap();
        InstalledCollectionPreparer::new(
            selection,
            ready,
            vec![ActionBinding {
                plan_pin: "fixture@sha256:abc".into(),
                node: "choose".into(),
                task_ref: "choose-task".into(),
                purpose: "candidate-select".into(),
                cursor_node: None,
                result_artifact: "relay-envelope".into(),
            }],
            ("relay".into(), "act".into()),
            directory,
            client,
            now,
        )
        .unwrap()
    }
    #[test]
    fn matched_constructor_requires_exact_v3_or_v4_members_ready_and_full_suffix() {
        for (variant, prefix) in [
            ("original", 0),
            ("changed", 0),
            ("subset", 0),
            ("empty", 0),
            ("original", 1),
            ("original", 2),
            ("original", 3),
        ] {
            let environment = Arc::new(Environment {
                now: AtomicU64::new(parse_selected_utc_ms("2026-11-04T10:00:00.100Z").unwrap()),
                revoked: AtomicBool::new(false),
            });
            let root = std::env::temp_dir().join(format!(
                "matched-constructor-{}",
                trollm_core::RunDir::under(Path::new("unused")).run_id()
            ));
            std::fs::create_dir_all(&root).unwrap();
            let mut body = crate::collection::tests::body();
            body["contextRef"]["sha256"] = json!(raw_sha256(b"controlled source context"));
            body["variant"] = json!(variant);
            if variant == "changed" {
                body["items"][0]["quantity"] = json!(3);
                body["items"][0]["quantityText"] = json!("3");
            }
            if variant == "subset" {
                body["items"] = json!([body["items"][1].clone()]);
                body["items"][0]["admittedOrder"] = json!(0);
            }
            if variant == "empty" {
                body["items"] = json!([]);
            }
            if prefix > 0 {
                body["continuation"]["prefixLength"] = json!(prefix);
                body["continuation"]["knownCommittedItemIds"] =
                    json!(body["items"].as_array().unwrap()[..prefix]
                        .iter()
                        .map(|item| item["originalItemId"].clone())
                        .collect::<Vec<_>>());
                body["continuation"]["parentOrigin"] = json!("offline-declared");
                body["continuation"]["parentCheckpointRef"] = body["applicationRef"].clone();
            }
            let selection = crate::collection::tests::decode(&body).unwrap();
            let decode = || {
                SelectionRecord::decode(
                    &selection.original_bytes,
                    &selection.selection_ref,
                    &selection.selection.owner,
                )
                .unwrap()
            };
            let client = || {
                PrivateGuardClient::new(
                    "https://localhost:9/guard",
                    Arc::new(|| Some("controlled-private-token".into())),
                    RootCertificates::from_pem(tls::ROOT_1_PEM).unwrap(),
                )
                .unwrap()
            };
            let mut ready = fixture(
                "https://localhost:9/guard".into(),
                environment.clone(),
                root.join("run"),
            )
            .ready;
            ready["schema"] = json!(crate::collection_members::MATCHED_READY);
            ready["nativeProfileId"] = json!(crate::collection_members::MATCHED_NATIVE_PROFILE);
            ready["navigationSupported"] = json!(true);
            ready["selection"] = body;
            ready["selectionRawSha256"] = json!(raw_sha256(&selection.original_bytes));
            ready["selectionRef"] = serde_json::to_value(&selection.selection_ref).unwrap();
            ready["currentUrl"] = json!(format!("http://127.0.0.1:4321/collection/{}", prefix + 1));
            let members = crate::collection_members::CollectionMembers::compile_matched(
                &selection,
                b"controlled source context",
                "https://api.e2llm.dev/mcp",
            )
            .unwrap();
            let directory = root.join("members");
            members.write_new(&directory).unwrap();
            let mut inspected = crate::collection_members::CollectionMembers::inspect_matched(
                &directory,
                &selection,
                b"controlled source context",
                "https://api.e2llm.dev/mcp",
            )
            .unwrap();
            let provider = InstalledCollectionPreparer::new_matched(
                decode(),
                ready.clone(),
                &inspected,
                ("e2llm".into(), "act".into()),
                root.join("run"),
                client(),
                environment.clone(),
            )
            .unwrap();
            assert_eq!(
                provider.expected_snapshot.as_ref().unwrap().len(),
                selection.selection.items.len() - prefix
            );
            assert_eq!(
                provider.evidence_provider(),
                "rpa-installed-pending-selected-v3"
            );
            let workload = crate::collection_members::CollectionMembers::compile_matched_600(
                &selection,
                b"controlled source context",
                "https://api.e2llm.dev/mcp",
            )
            .unwrap();
            let workload_directory = root.join("members-600");
            workload.write_new(&workload_directory).unwrap();
            let workload_inspected =
                crate::collection_members::CollectionMembers::inspect_matched_600(
                    &workload_directory,
                    &selection,
                    b"controlled source context",
                    "https://api.e2llm.dev/mcp",
                )
                .unwrap();
            // Native profile is selected by exact inspected members, not a free ready override.
            assert!(InstalledCollectionPreparer::new_matched(
                decode(),
                ready.clone(),
                &workload_inspected,
                ("e2llm".into(), "act".into()),
                root.join("run"),
                client(),
                environment.clone()
            )
            .is_err());
            let mut workload_ready = ready.clone();
            workload_ready["nativeProfileId"] =
                json!(crate::collection_members::WORKLOAD_600_NATIVE_PROFILE);
            assert!(InstalledCollectionPreparer::new_matched(
                decode(),
                workload_ready.clone(),
                &inspected,
                ("e2llm".into(), "act".into()),
                root.join("run"),
                client(),
                environment.clone()
            )
            .is_err());
            let workload_provider = InstalledCollectionPreparer::new_matched(
                decode(),
                workload_ready,
                &workload_inspected,
                ("e2llm".into(), "act".into()),
                root.join("run"),
                client(),
                environment.clone(),
            )
            .unwrap();
            assert_eq!(
                workload_provider.evidence_provider(),
                "rpa-installed-pending-selected-v4"
            );
            if !inspected.expected_snapshot.is_empty() {
                let run = root.join("run");
                std::fs::create_dir_all(run.join("artifacts")).unwrap();
                let written = trollm_core::artifacts::write_foreach_snapshot(
                    &run.join("artifacts"),
                    "items",
                    &inspected.expected_snapshot,
                )
                .unwrap();
                let path = written.path;
                let raw = std::fs::read(&path).unwrap();
                let snapshot_ref =
                    format!("artifacts/{}", path.file_name().unwrap().to_str().unwrap());
                let mut log =
                    trollm_core::EventLog::open(&run.join("events.jsonl"), "run").unwrap();
                log.append(
                    trollm_core::EventType::ArtifactWritten,
                    json!({"path":path,"bytes":raw.len(),"sha256":raw_sha256(&raw)}),
                )
                .unwrap();
                let first = &inspected.bindings[0];
                let mut actual = context();
                actual.run_id = "run".into();
                actual.node = first.node.clone();
                actual.task_ref = first.task_ref.clone();
                actual.frames = vec![
                    trollm_mcp::preparation::FramePosition {
                        plan: inspected.root_plan_pin.clone(),
                        node: "items".into(),
                    },
                    trollm_mcp::preparation::FramePosition {
                        plan: first.plan_pin.clone(),
                        node: first.node.clone(),
                    },
                ];
                actual.cursors = vec![trollm_mcp::preparation::CursorPosition {
                    node: "items".into(),
                    snapshot_ref: snapshot_ref.clone(),
                    index: 0,
                    phase: "inBody".into(),
                }];
                let call = ToolCall {
                    server: "e2llm".into(),
                    tool: "act".into(),
                    args: json!({"action":"click"}),
                };
                let request = provider.item_request(&call, &actual).unwrap();
                let mut response = guard(&ready, &request);
                response["item"] =
                    serde_json::to_value(&selection.selection.items[prefix]).unwrap();
                response["seededPrefixCount"] = json!(prefix);
                response["knownCommittedPrefixCount"] = json!(prefix);
                provider
                    .check_guard(&response, &request, &actual, true)
                    .unwrap();
                response["schema"] = json!("controlled-collection-selected-guard.v2");
                assert!(provider
                    .check_guard(&response, &request, &actual, true)
                    .is_err());
            }
            assert!(InstalledCollectionPreparer::new(
                decode(),
                ready.clone(),
                inspected.bindings.clone(),
                ("e2llm".into(), "act".into()),
                root.join("run"),
                client(),
                environment.clone()
            )
            .is_err());
            for (key, value) in [
                ("schema", json!("controlled-collection-selected-ready.v2")),
                (
                    "nativeProfileId",
                    json!("controlled-collection-native-selected.v2-proposal"),
                ),
                ("navigationSupported", json!(false)),
            ] {
                let mut wrong = ready.clone();
                wrong[key] = value;
                assert!(InstalledCollectionPreparer::new_matched(
                    decode(),
                    wrong,
                    &inspected,
                    ("e2llm".into(), "act".into()),
                    root.join("run"),
                    client(),
                    environment.clone()
                )
                .is_err());
            }
            inspected.member_profile = crate::collection_members::N1_OFFER_PROFILE.into();
            assert!(InstalledCollectionPreparer::new_matched(
                decode(),
                ready.clone(),
                &inspected,
                ("e2llm".into(), "act".into()),
                root.join("run"),
                client(),
                environment.clone()
            )
            .is_err());
            inspected.member_profile = crate::collection_members::MATCHED_PROFILE.into();
            if !inspected.expected_snapshot.is_empty() {
                inspected.expected_snapshot[0]["value"]["quantityText"] = json!("invalid");
                assert!(InstalledCollectionPreparer::new_matched(
                    decode(),
                    ready,
                    &inspected,
                    ("e2llm".into(), "act".into()),
                    root.join("run"),
                    client(),
                    environment.clone()
                )
                .is_err());
            }
            std::fs::remove_dir_all(root).unwrap();
        }
    }
    fn context() -> PreparationContext {
        PreparationContext {
            run_id: "fixture-run".into(),
            node: "choose".into(),
            task_ref: "choose-task".into(),
            origin: trollm_mcp::preparation::CallOrigin::Model {
                slot: "coder".into(),
                ordinal: 1,
            },
            frames: vec![trollm_mcp::preparation::FramePosition {
                plan: "fixture@sha256:abc".into(),
                node: "choose".into(),
            }],
            cursors: vec![],
            argument_bindings: vec![],
            call_context: CallContext {
                epoch: "actual-controlled-epoch".into(),
                attempt: u64::MAX,
                call: 1,
            },
        }
    }
    fn guard(ready: &Value, request: &Value) -> Value {
        json!({"schema":if ready["schema"]==crate::collection_members::MATCHED_READY { crate::collection_members::MATCHED_GUARD } else { "controlled-collection-selected-guard.v2" },"invocationKey":request["invocationKey"],
            "selectionRef":request["selectionRef"],"selectionRawSha256":request["selectionRawSha256"],"owner":request["owner"],
            "selectionId":request["selectionId"],"caseId":request["caseId"],"callerContext":request["callerContext"],
            "callerAssociation":request["callerAssociation"],"purpose":request["purpose"],"item":ready["selection"]["items"][0],
            "browser":"fixture-browser","tabId":3,"currentUrl":ready["currentUrl"],"sifrSessionId":"actual-capture-SID",
            "nodeId":"actual-node-id","selector":"[data-control=fixture-target]",
            "args":{"action":"click","browser":"fixture-browser","sessionId":"actual-capture-SID","target":"[data-control=fixture-target]"},
            "seededPrefixCount":0,"knownCommittedPrefixCount":0,"newCommittedAdds":0,
            "observationStartedAt":"2026-11-04T09:59:59.999Z","observationFinishedAt":"2026-11-04T10:00:00.050Z",
            "effectiveExpiresAt":"2026-11-04T10:00:00.999Z","maxObservationAgeMs":1000,"minPreparationRemainingMs":100,
            "clockPolicy":ready["preparationPolicy"]["clockPolicy"],
            "clockVerification":{"verified":true,"assuranceRef":ready["preparationPolicy"]["clockPolicy"]["assuranceRef"],
                "rawSha256":ready["preparationPolicy"]["clockPolicy"]["assuranceRef"]["sha256"],"maxClockErrorMs":1,
                "validFrom":"2026-11-04T09:59:59.900Z","validUntil":"2026-11-04T10:00:00.050Z"},
            "observationAcquisitions":[{"kind":"controlled-fixture","startedAt":"2026-11-04T10:00:00.000Z",
                "finishedAt":"2026-11-04T10:00:00.050Z","elapsedMonotonicMs":50.0}],
            "installerClock":{"source":"RPA38c-kernel-clock-assurance","invalid":false,"units":"milliseconds-since-unix-epoch",
                "clockDomain":{"kind":"linux-CLOCK_REALTIME","bootId":"controlled-fixture"},
                "lastSample":{"kernelAssured":true,"finishedAt":"2026-11-04T10:00:00.050Z"}},
            "originalExpiryEvidence":{"caseExpiresAt":ready["originalCaseExpiresAt"],"selectionExpiresAt":ready["preparationPolicy"]["selectionExpiresAt"],
                "authorityExpiresAt":ready["preparationPolicy"]["authorityExpiresAt"],
                "supportRef":ready["policyEvidence"]["ref"],"supportRawSha256":ready["policyEvidence"]["rawSha256"],
                "supplied":ready["expiryEvidence"]}})
    }
    #[test]
    fn inspected_suffix_requires_exact_native_snapshot_and_original_artifact_record() {
        let base = std::env::temp_dir().join(format!(
            "selected-snapshot-{}",
            trollm_core::RunDir::under(Path::new("unused")).run_id()
        ));
        let run = base.join("fixture-run");
        std::fs::create_dir_all(run.join("artifacts")).unwrap();
        let now = Arc::new(Environment {
            now: AtomicU64::new(1),
            revoked: AtomicBool::new(false),
        });
        let mut provider = fixture("https://localhost.invalid/guard".into(), now, run.clone());
        provider.bindings[0].cursor_node = Some("items".into());
        let mut context = context();
        context
            .cursors
            .push(trollm_mcp::preparation::CursorPosition {
                node: "items".into(),
                snapshot_ref: String::new(),
                index: 0,
                phase: "inBody".into(),
            });
        let call = ToolCall {
            server: "relay".into(),
            tool: "act".into(),
            args: json!({"action":"click"}),
        };
        assert!(provider.item_request(&call, &context).is_err());
        let prepared = trollm_mcp::local::prepare_worklist(&json!({"input":serde_json::to_string(&json!({"context":"controlled context","items":provider.selection.selection.items})).unwrap()})).unwrap();
        let inspected = crate::collection_members::InspectedCollectionMembers {
            member_profile: crate::collection_members::PROFILE.into(),
            root_plan_pin: context.frames[0].plan.clone(),
            bindings: provider.bindings.clone(),
            expected_snapshot: prepared["items"].as_array().unwrap().clone(),
            selection_input_json: String::new(),
        };
        let provider = provider.with_inspected_snapshot(&inspected).unwrap();
        let written = trollm_core::artifacts::write_foreach_snapshot(
            &run.join("artifacts"),
            "items",
            &inspected.expected_snapshot,
        )
        .unwrap();
        let path = written.path;
        let raw = String::from_utf8(std::fs::read(&path).unwrap()).unwrap();
        let snapshot_ref = format!("artifacts/{}", path.file_name().unwrap().to_str().unwrap());
        context.cursors[0].snapshot_ref = snapshot_ref.clone();
        let mut log =
            trollm_core::EventLog::open(&run.join("events.jsonl"), "fixture-run").unwrap();
        assert!(
            provider.item_request(&call, &context).is_err(),
            "no source-supplied snapshot without native record"
        );
        log.append(
            trollm_core::EventType::ArtifactWritten,
            json!({"path":path,"bytes":raw.len(),"sha256":raw_sha256(raw.as_bytes())}),
        )
        .unwrap();
        assert_eq!(
            provider.item_request(&call, &context).unwrap()["originalItemId"],
            provider.selection.selection.items[0].original_item_id
        );
        for changed in [format!(" {raw}"), format!("{raw}{raw}"), String::new()] {
            std::fs::write(&path, changed).unwrap();
            assert!(provider.item_request(&call, &context).is_err());
        }
        std::fs::write(&path, &raw).unwrap();
        for dirty in [
            "artifacts/items.snapshot.jsonl".to_owned(),
            format!("artifacts/items.{}.snapshot.jsonl", "f".repeat(64)),
            "../other.snapshot.jsonl".to_owned(),
        ] {
            context.cursors[0].snapshot_ref = dirty;
            assert!(provider.item_request(&call, &context).is_err());
        }
        context.cursors[0].snapshot_ref = snapshot_ref;
        log.append(
            trollm_core::EventType::ArtifactWritten,
            json!({"path":path,"bytes":raw.len(),"sha256":"f".repeat(64)}),
        )
        .unwrap();
        assert!(
            provider.item_request(&call, &context).is_err(),
            "last committed identity must match"
        );
        log.append(
            trollm_core::EventType::ArtifactWritten,
            json!({"path":path,"bytes":raw.len(),"sha256":raw_sha256(raw.as_bytes())}),
        )
        .unwrap();
        assert!(
            provider.item_request(&call, &context).is_err(),
            "later records cannot repair conflicting immutable commitments"
        );
        context.frames[0].plan = "different@sha256:controlled".into();
        assert!(provider.item_request(&call, &context).is_err());
        drop(log);
        std::fs::remove_dir_all(base).unwrap();
    }
    #[test]
    fn navigation_source_checks_completed_cursor_last_item_and_distinct_url_target() {
        // Exercise source-side validators only. The legacy constructor still
        // refuses navigation installation and ready flags are not authority.
        for prefix in 0..3 {
            let base = std::env::temp_dir().join(format!(
                "selected-nav-control-{}",
                trollm_core::RunDir::under(Path::new("unused")).run_id()
            ));
            let run = base.join("fixture-run");
            std::fs::create_dir_all(run.join("artifacts")).unwrap();
            let now = parse_selected_utc_ms("2026-11-04T10:00:00.100Z").unwrap();
            let env = Arc::new(Environment {
                now: AtomicU64::new(now),
                revoked: AtomicBool::new(false),
            });
            let mut provider = fixture("https://localhost:1/guard".into(), env, run.clone());
            let mut body = crate::collection::tests::body();
            if prefix != 0 {
                body["continuation"]["prefixLength"] = json!(prefix);
                body["continuation"]["knownCommittedItemIds"] =
                    json!(body["items"].as_array().unwrap()[..prefix]
                        .iter()
                        .map(|item| item["originalItemId"].clone())
                        .collect::<Vec<_>>());
                body["continuation"]["parentOrigin"] = json!("offline-declared");
                body["continuation"]["parentCheckpointRef"] = body["applicationRef"].clone();
            }
            provider.selection = crate::collection::tests::decode(&body).unwrap();
            provider.ready["selection"] = body;
            provider.ready["currentUrl"] =
                json!(format!("http://127.0.0.1:4321/collection/{}", prefix + 1));
            provider.bindings[0] = ActionBinding {
                plan_pin: "between@sha256:controlled".into(),
                node: "navigate".into(),
                task_ref: "navigate-next".into(),
                purpose: "navigate-next".into(),
                cursor_node: Some("items".into()),
                result_artifact: "relay-envelope".into(),
            };
            let mut context = context();
            context.node = "navigate".into();
            context.task_ref = "navigate-next".into();
            context.frames = vec![
                trollm_mcp::preparation::FramePosition {
                    plan: "root@sha256:controlled".into(),
                    node: "items".into(),
                },
                trollm_mcp::preparation::FramePosition {
                    plan: "between@sha256:controlled".into(),
                    node: "navigate".into(),
                },
            ];
            context.cursors = vec![trollm_mcp::preparation::CursorPosition {
                node: "items".into(),
                snapshot_ref: String::new(),
                index: 0,
                phase: "inBetween".into(),
            }];
            let prepared = trollm_mcp::local::prepare_worklist(&json!({"input":serde_json::to_string(
                &json!({"context":"controlled context","items":&provider.selection.selection.items[prefix..]})).unwrap()})).unwrap();
            let inspected = crate::collection_members::InspectedCollectionMembers {
                member_profile: crate::collection_members::PROFILE.into(),
                root_plan_pin: context.frames[0].plan.clone(),
                bindings: provider.bindings.clone(),
                expected_snapshot: prepared["items"].as_array().unwrap().clone(),
                selection_input_json: String::new(),
            };
            let mut provider = provider.with_inspected_snapshot(&inspected).unwrap();
            let written = trollm_core::artifacts::write_foreach_snapshot(
                &run.join("artifacts"),
                "items",
                &inspected.expected_snapshot,
            )
            .unwrap();
            let path = written.path;
            let bytes = std::fs::read(&path).unwrap();
            context.cursors[0].snapshot_ref =
                format!("artifacts/{}", path.file_name().unwrap().to_str().unwrap());
            let mut log =
                trollm_core::EventLog::open(&run.join("events.jsonl"), "fixture-run").unwrap();
            log.append(
                trollm_core::EventType::ArtifactWritten,
                json!({"path":path,"bytes":bytes.len(),"sha256":raw_sha256(&bytes)}),
            )
            .unwrap();
            let call = ToolCall {
                server: "relay".into(),
                tool: "act".into(),
                args: json!({"action":"navigate"}),
            };
            assert!(
                provider.item_request(&call, &context).is_err(),
                "released navigation=false must refuse"
            );
            provider.ready["navigationSupported"] = json!(true); // Declared validator vector only.
            for index in 0..(3 - prefix) {
                context.cursors[0].index = index as u64;
                let request = provider.item_request(&call, &context);
                if prefix + index == 2 {
                    assert!(
                        request.is_err(),
                        "last item must refuse before effect preparation"
                    );
                    continue;
                }
                let request = request.unwrap();
                assert_eq!(request["admittedOrder"], prefix + index);
                let mut response = guard(&provider.ready, &request);
                response["item"] =
                    serde_json::to_value(&provider.selection.selection.items[prefix + index])
                        .unwrap();
                response["currentUrl"] =
                    json!(provider.selected_page_url((prefix + index) as u64).unwrap());
                response["knownCommittedPrefixCount"] = json!(prefix + index + 1);
                response["seededPrefixCount"] = json!(prefix);
                response["newCommittedAdds"] = json!(index + 1);
                response["selector"] = json!("[data-control=catalog-link]");
                response["fixturePurpose"] = json!("catalog-link");
                response["args"]["action"] = json!("navigate");
                response["args"]["target"] = json!(provider
                    .selected_page_url((prefix + index + 1) as u64)
                    .unwrap());
                response["effectiveExpiresAt"] = json!("2026-11-04T10:00:00.500Z");
                provider
                    .check_guard(&response, &request, &context, true)
                    .unwrap();
                for wrong_purpose in [Value::Null, json!("navigate-next")] {
                    let mut changed = response.clone();
                    changed["fixturePurpose"] = wrong_purpose;
                    assert!(provider
                        .check_guard(&changed, &request, &context, false)
                        .is_err());
                }
                for wrong in [
                    json!("http://127.0.0.1:4321/collection/99"),
                    json!("http://127.0.0.1:9999/collection/2"),
                    json!("[data-control=catalog-link]"),
                ] {
                    let mut changed = response.clone();
                    changed["args"]["target"] = wrong;
                    assert!(provider
                        .check_guard(&changed, &request, &context, false)
                        .is_err());
                }
                let mut wrong = response.clone();
                wrong["knownCommittedPrefixCount"] = request["admittedOrder"].clone();
                assert!(provider
                    .check_guard(&wrong, &request, &context, false)
                    .is_err());
                context.cursors[0].phase = "inBody".into();
                assert!(provider.item_request(&call, &context).is_err());
                context.cursors[0].phase = "inBetween".into();
            }
            drop(log);
            std::fs::remove_dir_all(base).unwrap();
        }
    }

    #[test]
    fn empty_selection_allows_zero_bindings_but_cannot_prepare_an_effect() {
        // Declared source vectors only, not retained owned artifacts/admission.
        let now = parse_selected_utc_ms("2026-11-04T10:00:00.100Z").unwrap();
        let env = Arc::new(Environment {
            now: AtomicU64::new(now),
            revoked: AtomicBool::new(false),
        });
        let base = fixture(
            "https://localhost:1/guard".into(),
            env.clone(),
            PathBuf::from("/private/fixture-run"),
        );
        let mut body = crate::collection::tests::body();
        body["variant"] = json!("empty");
        body["items"] = json!([]);
        let selection = crate::collection::tests::decode(&body).unwrap();
        let mut ready = base.ready.clone();
        ready["selection"] = body;
        ready["selectionRef"] = serde_json::to_value(&selection.selection_ref).unwrap();
        ready["selectionRawSha256"] = json!(raw_sha256(&selection.original_bytes));
        let provider = InstalledCollectionPreparer::new(
            selection,
            ready,
            vec![],
            ("relay".into(), "act".into()),
            PathBuf::from("/private/fixture-run"),
            base.client,
            env,
        )
        .unwrap();
        let call = ToolCall {
            server: "relay".into(),
            tool: "act".into(),
            args: json!({"action":"click"}),
        };
        let error = provider
            .prepare(
                &call,
                &context(),
                Some(Instant::now() + Duration::from_secs(1)),
            )
            .err()
            .unwrap();
        assert!(error.to_string().contains("unmapped selected action"));
    }

    #[test]
    fn selected_policy_association_final_expiry_and_current_authority_refuse_drift() {
        let now = parse_selected_utc_ms("2026-11-04T10:00:00.100Z").unwrap();
        let env = Arc::new(Environment {
            now: AtomicU64::new(now),
            revoked: AtomicBool::new(false),
        });
        let provider = fixture(
            "https://localhost:1/guard".into(),
            env.clone(),
            PathBuf::from("/private/fixture-run"),
        );
        let context = context();
        let call = ToolCall {
            server: "relay".into(),
            tool: "act".into(),
            args: json!({"action":"click"}),
        };
        let request = provider.item_request(&call, &context).unwrap();
        let good = guard(&provider.ready, &request);
        provider
            .check_guard(&good, &request, &context, true)
            .unwrap();
        // A matched original navigation arm may attenuate, never renew, the
        // effective cutoff. The preparation reserve is not a send reserve.
        let mut shortened = good.clone();
        shortened["effectiveExpiresAt"] = json!("2026-11-04T10:00:00.500Z");
        provider
            .check_guard(&shortened, &request, &context, true)
            .unwrap();
        shortened["effectiveExpiresAt"] = json!("2026-11-04T10:00:00.199Z");
        assert!(provider
            .check_guard(&shortened, &request, &context, true)
            .is_err());
        provider
            .check_guard(&shortened, &request, &context, false)
            .unwrap();
        shortened["effectiveExpiresAt"] = json!("2026-11-04T10:00:01.000Z");
        assert!(provider
            .check_guard(&shortened, &request, &context, false)
            .is_err());
        for key in [
            "caseId",
            "selectionRawSha256",
            "callerContext",
            "owner",
            "item",
            "effectiveExpiresAt",
            "args",
            "tabId",
            "clockVerification",
            "observationAcquisitions",
            "installerClock",
            "originalExpiryEvidence",
        ] {
            let mut wrong = good.clone();
            wrong[key] = Value::Null;
            assert!(
                provider
                    .check_guard(&wrong, &request, &context, false)
                    .is_err(),
                "{key}"
            );
        }
        env.now.store(now + 850, Ordering::SeqCst);
        assert!(
            provider
                .check_guard(&good, &request, &context, true)
                .is_err(),
            "preparation reserve consumed"
        );
        provider
            .check_guard(&good, &request, &context, false)
            .unwrap(); // Reserve is not reapplied at send.
        env.now.store(now + 901, Ordering::SeqCst);
        assert!(provider
            .check_guard(&good, &request, &context, false)
            .is_err());
        env.now.store(now, Ordering::SeqCst);
        env.revoked.store(true, Ordering::SeqCst);
        assert!(provider
            .check_guard(&good, &request, &context, false)
            .is_err());
    }
    #[test]
    fn actual_tls_prepare_preserves_proposal_context_and_resolves_measured_selector() {
        use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!(
            "https://localhost:{}/guard",
            listener.local_addr().unwrap().port()
        );
        let now = parse_selected_utc_ms("2026-11-04T10:00:00.100Z").unwrap();
        let env = Arc::new(Environment {
            now: AtomicU64::new(now),
            revoked: AtomicBool::new(false),
        });
        let parent = std::env::temp_dir().join(format!(
            "native-provider-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let run = parent.join("fixture-run");
        std::fs::create_dir_all(run.join("artifacts")).unwrap();
        let provider = fixture(url, env, run.clone());
        let ready = provider.ready.clone();
        let server = std::thread::spawn(move || {
            let config = rustls::ServerConfig::builder_with_provider(Arc::new(
                rustls::crypto::ring::default_provider(),
            ))
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(
                vec![CertificateDer::from(tls::LEAF_1_DER.to_vec())],
                PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(tls::LEAF_1_KEY_DER.to_vec())),
            )
            .unwrap();
            let deadline = Instant::now() + Duration::from_secs(3);
            let mut outcomes = Vec::new();
            let mut turn = 0;
            while turn < 3 {
                let socket = loop {
                    match listener.accept() {
                        Ok((socket, _)) => break socket,
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            assert!(Instant::now() < deadline);
                            std::thread::sleep(Duration::from_millis(5));
                        }
                        Err(error) => panic!("{error}"),
                    }
                };
                socket
                    .set_read_timeout(Some(Duration::from_secs(3)))
                    .unwrap();
                socket
                    .set_write_timeout(Some(Duration::from_secs(3)))
                    .unwrap();
                let stream = rustls::StreamOwned::new(
                    rustls::ServerConnection::new(Arc::new(config.clone())).unwrap(),
                    socket,
                );
                let mut reader = BufReader::new(stream);
                let mut length = 0;
                let mut is_control = false;
                loop {
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    is_control |= line.starts_with("GET /control ");
                    if line == "\r\n" {
                        break;
                    }
                    if let Some((name, value)) = line.split_once(':') {
                        if name.eq_ignore_ascii_case("content-length") {
                            length = value.trim().parse::<usize>().unwrap();
                        }
                    }
                }
                if is_control {
                    let reply = serde_json::to_vec(&json!({"schema":"trollm.private-guard-control.v1",
                        "invocationKey":ready["invocationKey"],"aborted":false,"expiresAt":ready["expiresAt"]})).unwrap();
                    write!(
                        reader.get_mut(),
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        reply.len()
                    )
                    .unwrap();
                    reader.get_mut().write_all(&reply).unwrap();
                    reader.get_mut().flush().unwrap();
                    continue;
                }
                assert!(length <= 262144);
                let mut bytes = vec![0; length];
                reader.read_exact(&mut bytes).unwrap();
                let request: Value = serde_json::from_slice(&bytes).unwrap();
                assert_eq!(
                    request["nativeContext"]["callContext"]["attempt"],
                    "18446744073709551615"
                );
                let response = if turn == 0 {
                    serde_json::to_vec(&guard(&ready, &request["guardRequest"])).unwrap()
                } else {
                    assert_eq!(request["guardRequest"]["kind"], "outcome");
                    assert_eq!(request["guardRequest"]["outcome"], "OBSERVATION_RETURNED");
                    outcomes.push(bytes.clone());
                    serde_json::to_vec(&json!({"recorded":true,"source":"native-adapter-reported-not-independent-settlement"})).unwrap()
                };
                write!(reader.get_mut(),"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nx-native-context-sha256: {}\r\nx-response-sha256: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                request["nativeContext"]["sha256"].as_str().unwrap(),raw_sha256(&response),response.len()).unwrap();
                reader.get_mut().write_all(&response).unwrap();
                reader.get_mut().flush().unwrap();
                turn += 1;
            }
            assert_eq!(
                outcomes[0], outcomes[1],
                "report retry reuses exact original HTTP bytes"
            );
        });
        let context = context();
        let call = ToolCall {
            server: "relay".into(),
            tool: "act".into(),
            args: json!({"action":"click","target":"model-proposed-target"}),
        };
        let prepared = provider
            .prepare(
                &call,
                &context,
                Some(Instant::now() + Duration::from_secs(3)),
            )
            .unwrap();
        assert_eq!(
            prepared.call.args["target"],
            "[data-control=fixture-target]"
        );
        assert_eq!(call.args["target"], "model-proposed-target");
        assert!(
            provider
                .acquire_before_send(&prepared.call, &context, &prepared.evidence)
                .is_err(),
            "guard response alone is not retained native intent"
        );
        let artifact = run.join("artifacts").join(
            trollm_mcp::preparation::selected_result_artifact_name("relay-envelope", &context)
                .unwrap(),
        );
        let envelope = json!({"result":"mutation","actionClassification":"mutation-observed"});
        let raw = serde_json::to_vec(&envelope).unwrap();
        std::fs::write(&artifact, &raw).unwrap();
        std::fs::write(
            format!("{}.meta.json", artifact.display()),
            b"{\"complete\":true}",
        )
        .unwrap();
        let result = ToolResult {
            content_for_model: envelope,
            artifacts: vec![artifact.clone()],
            provenance: trollm_mcp::Provenance::Live,
        };
        let mut log =
            trollm_core::EventLog::open(&run.join("events.jsonl"), "fixture-run").unwrap();
        let common = json!({"node":context.node,"taskRef":context.task_ref,"callContext":context.call_context,"server":"relay","tool":"act"});
        log.append(trollm_core::EventType::ToolCallPrepared,
            json!({"node":context.node,"taskRef":context.task_ref,"callContext":context.call_context,
                "resolvedCall":prepared.call,"nativeContext":context.raw().unwrap(),"evidence":prepared.evidence})).unwrap();
        let mut intent = common.clone();
        intent["args"] = prepared.call.args.clone();
        intent["argsSha256"] = json!(raw_sha256(
            &serde_json::to_vec(&prepared.call.args).unwrap()
        ));
        let i = log
            .append(trollm_core::EventType::ToolCallIntent, intent)
            .unwrap();
        provider
            .after_retained_intent(&prepared.call, &context, &prepared.evidence, i)
            .unwrap();
        provider
            .acquire_before_send(&prepared.call, &context, &prepared.evidence)
            .unwrap();
        provider
            .check_before_send(&prepared.call, &context, &prepared.evidence)
            .unwrap();
        let mut returned = common;
        returned["intentSeq"] = json!(i);
        returned["resultArtifact"] = Value::Null;
        returned["artifacts"] = json!([artifact]);
        returned["argsHash"] = json!(trollm_mcp::CacheStore::hash_args(
            &prepared.call.args,
            None,
            None
        ));
        let r = log
            .append(trollm_core::EventType::ToolCallResult, returned)
            .unwrap();
        log.append(
            trollm_core::EventType::ArtifactWritten,
            json!({"path":artifact,"sha256":raw_sha256(&raw),"bytes":raw.len()}),
        )
        .unwrap();
        provider
            .after_retained_result(
                &prepared.call,
                &context,
                &prepared.evidence,
                &result,
                Some(i),
                r,
            )
            .unwrap();
        provider
            .after_retained_result(
                &prepared.call,
                &context,
                &prepared.evidence,
                &result,
                Some(i),
                r,
            )
            .unwrap();
        assert!(run
            .join(format!(
                "selected-report/{}.ack.json",
                context.raw().unwrap().sha256
            ))
            .is_file());
        server.join().unwrap();
        drop(log);
        let mut altered = trollm_core::EventLog::read_all(&run.join("events.jsonl")).unwrap();
        let rewritten = altered
            .iter()
            .map(|record| format!("{}\n", record))
            .collect::<String>();
        std::fs::write(run.join("events.jsonl"), rewritten).unwrap();
        let error = provider
            .acquire_before_send(&prepared.call, &context, &prepared.evidence)
            .unwrap_err();
        assert!(
            error.to_string().contains("bytes changed"),
            "logical reserialization must not replace original record bytes"
        );
        let intent = altered
            .iter_mut()
            .find(|record| record["type"] == "tool_call_intent")
            .unwrap();
        intent["args"]["target"] = json!("changed-after-native-intent");
        intent["argsSha256"] = json!(raw_sha256(&serde_json::to_vec(&intent["args"]).unwrap()));
        let bytes = altered
            .iter()
            .map(|record| format!("{}\n", record))
            .collect::<String>();
        std::fs::write(run.join("events.jsonl"), bytes).unwrap();
        assert!(
            provider
                .acquire_before_send(&prepared.call, &context, &prepared.evidence)
                .is_err(),
            "final write must recheck original retained intent, not trust a prior flag"
        );
        std::fs::remove_dir_all(parent).unwrap();
    }
}
