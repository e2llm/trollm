//! Explicit installed Android/KVM calibration profile. Not selected by source,
//! model arguments, framework confidence or a kernel synchronized flag.
use crate::collection::{raw_sha256, SelectionRecord};
use anyhow::{ensure, Context, Result};
use serde::Deserialize;
use serde_json::Value;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, AtomicU8, Ordering},
    Arc, Condvar, Mutex,
};
use std::time::{Duration, Instant};
use trollm_mcp::clock::{
    parse_selected_utc_ms, BootSample, ElapsedRate, ElapsedWitness, VerifiedUtcAnchor,
};
use trollm_mcp::private_guard::PrivateGuardClient;

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct CalibratedClockPolicy {
    pub profile_id: String,
    pub guest_boot_id: String,
    pub minimum_rate_numerator: u64,
    pub minimum_rate_denominator: u64,
    pub max_calibration_age_ms: u64,
}
impl CalibratedClockPolicy {
    pub fn validate(&self) -> Result<ElapsedRate> {
        ensure!(
            self.profile_id == "trollm.android-selected-emulator-calibrated-clock.v1-proposal"
                && (1..=30000).contains(&self.max_calibration_age_ms)
                && self.minimum_rate_denominator <= 9_007_199_254_740_991
                && self.guest_boot_id.len() == 36
                && self.guest_boot_id.bytes().enumerate().all(|(i, c)| {
                    if matches!(i, 8 | 13 | 18 | 23) {
                        c == b'-'
                    } else {
                        c.is_ascii_digit() || (b'a'..=b'f').contains(&c)
                    }
                }),
            "selected emulator calibration policy unavailable"
        );
        ElapsedRate::new(self.minimum_rate_numerator, self.minimum_rate_denominator)
    }
}
struct Calibration {
    anchor: VerifiedUtcAnchor,
    oldest_lower_ms: u64,
}
struct State {
    witness: ElapsedWitness,
    source_identity: Option<String>,
    calibration: Option<Calibration>,
    ordinal: u64,
}
pub(crate) struct CalibratedClock {
    policy: CalibratedClockPolicy,
    rate: ElapsedRate,
    client: PrivateGuardClient,
    invocation: String,
    owner: Value,
    selection: Value,
    assurance: Value,
    max_host_error_us: u64,
    original_expires_at: String,
    publication_cutoff: u64,
    evidence_directory: PathBuf,
    invalid: AtomicBool,
    failure: AtomicU8,
    acquisition: Mutex<()>,
    maintenance: Mutex<Option<Maintenance>>,
    state: Mutex<State>,
}
#[derive(Debug, Clone, Copy)]
pub(crate) struct ClockFailure(pub &'static str);
impl std::fmt::Display for ClockFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}
impl std::error::Error for ClockFailure {}
#[derive(Debug)]
pub(crate) struct ClockReadBusy;
impl std::fmt::Display for ClockReadBusy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SELECTED_CLOCK_READ_BUSY")
    }
}
impl std::error::Error for ClockReadBusy {}
struct Maintenance {
    wake: Arc<(Mutex<bool>, Condvar)>,
    worker: std::thread::JoinHandle<()>,
}
fn retain_original(directory: &Path, name: &str, bytes: &[u8]) -> Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(directory.join(name))?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}
impl CalibratedClock {
    pub fn new(
        policy: CalibratedClockPolicy,
        client: PrivateGuardClient,
        ready: &Value,
        selection: &SelectionRecord,
        publication_cutoff: u64,
        evidence_directory: PathBuf,
    ) -> Result<Self> {
        let rate = crate::selected_start::at(
            policy.validate(),
            "SELECTED_CLOCK_POLICY",
            &evidence_directory,
        )?;
        crate::selected_start::at(
            (|| -> Result<()> {
                ensure!(
                    BootSample::read()?.boot_id() == policy.guest_boot_id,
                    "installed emulator boot differs from original native boot"
                );
                Ok(())
            })(),
            "SELECTED_CLOCK_BOOT",
            &evidence_directory,
        )?;
        let witness = crate::selected_start::at(
            ElapsedWitness::read(&policy.guest_boot_id),
            "SELECTED_CLOCK_WITNESS",
            &evidence_directory,
        )?;
        let clock = &ready["preparationPolicy"]["clockPolicy"];
        let max_host_error_us = clock["maxClockErrorMs"]
            .as_u64()
            .and_then(|n| n.checked_mul(1000))
            .context("host clock policy unavailable")?;
        let original_expires_at = ready["expiresAt"]
            .as_str()
            .context("original actor expiry unavailable")?
            .to_owned();
        Ok(Self {
            policy,
            rate,
            client,
            invocation: selection.selection.invocation_key.clone(),
            owner: serde_json::to_value(&selection.selection.owner)?,
            selection: serde_json::to_value(&selection.selection_ref)?,
            assurance: clock["assuranceRef"].clone(),
            max_host_error_us,
            original_expires_at,
            publication_cutoff,
            evidence_directory,
            invalid: false.into(),
            failure: AtomicU8::new(0),
            acquisition: Mutex::new(()),
            maintenance: Mutex::new(None),
            state: Mutex::new(State {
                witness,
                source_identity: None,
                calibration: None,
                ordinal: 0,
            }),
        })
    }
    fn failure_code(&self) -> &'static str {
        match self.failure.load(Ordering::SeqCst) {
            1 => "SELECTED_CLOCK_STALE",
            2 => "SELECTED_CLOCK_DEADLINE",
            3 => "SELECTED_CLOCK_DISCONTINUITY",
            4 => "SELECTED_CLOCK_SOURCE_UNAVAILABLE",
            5 => "SELECTED_CLOCK_RETIRED",
            6 => "SELECTED_CLOCK_REFRESH_TIMEOUT",
            _ => "SELECTED_CLOCK_UNAVAILABLE",
        }
    }
    fn fence<T>(&self, result: Result<T>) -> Result<T> {
        result.map_err(|error| {
            let code = match error.downcast_ref::<trollm_mcp::clock::ClockMappingFailure>() {
                Some(trollm_mcp::clock::ClockMappingFailure::Stale) => 1,
                Some(trollm_mcp::clock::ClockMappingFailure::Expired) => 2,
                Some(trollm_mcp::clock::ClockMappingFailure::Discontinuity) => 3,
                None if error
                    .downcast_ref::<trollm_mcp::private_guard::GuardClockTimeout>()
                    .is_some() =>
                {
                    6
                }
                None => {
                    if let Some(failure) = error.downcast_ref::<ClockFailure>() {
                        match failure.0 {
                            "SELECTED_CLOCK_DISCONTINUITY" => 3,
                            _ => 4,
                        }
                    } else {
                        4
                    }
                }
            };
            let _ = self
                .failure
                .compare_exchange(0, code, Ordering::SeqCst, Ordering::SeqCst);
            self.invalid.store(true, Ordering::SeqCst);
            ClockFailure(self.failure_code()).into()
        })
    }
    /// Network and custody occur only here, before bounded local admission.
    pub fn acquire(&self) -> Result<()> {
        self.fence((|| {
            ensure!(!self.invalid.load(Ordering::SeqCst), "calibrated clock permanently fenced");
            // Serialize acquisitions independently. Network/custody must never
            // hold the state mutex needed by model-control or final local reads.
            let _acquisition = self.acquisition.lock().map_err(|_| anyhow::anyhow!("clock acquisition unavailable"))?;
            let (ordinal, budget_ms) = {
                let mut state = self.state.lock().map_err(|_| anyhow::anyhow!("clock state unavailable"))?;
                state.witness = state.witness.intersect(&ElapsedWitness::read(&self.policy.guest_boot_id)?)
                    .map_err(|_| ClockFailure("SELECTED_CLOCK_DISCONTINUITY"))?;
                let budget_ms = if let Some(previous) = &state.calibration {
                    previous.anchor.remaining_validity_ms(&state.witness.boot)?
                } else { self.policy.max_calibration_age_ms };
                state.ordinal = state.ordinal.checked_add(1).context("calibration ordinal overflow")?;
                (state.ordinal, budget_ms)
            };
            let guest_budget_ns = (u128::from(budget_ms) * u128::from(self.policy.minimum_rate_numerator) * 1_000_000)
                / u128::from(self.policy.minimum_rate_denominator);
            ensure!(guest_budget_ns > 0 && guest_budget_ns <= u128::from(u64::MAX), "calibration request budget exhausted");
            let deadline = Instant::now().checked_add(Duration::from_nanos(guest_budget_ns as u64)).context("calibration request deadline overflow")?;
            let response = self.client.acquire_clock(&self.invocation, &self.policy.guest_boot_id,
                &self.original_expires_at, deadline,
                &|| {
                    ensure!(!self.invalid.load(Ordering::SeqCst), "calibration invalidated");
                    Ok(())
                })?;
            let name = format!("clock-calibration-{ordinal:06}");
            let brackets = serde_json::to_vec(&serde_json::json!({
                "schema":"trollm.clock-calibration-bracket.v1-proposal",
                "sent":response.sent,"received":response.received,
                "sentEndpoint":response.sent_endpoint,"receivedEndpoint":response.received_endpoint,
                "requestSha256":raw_sha256(&response.request_bytes),"responseSha256":response.sha256}))?;
            retain_original(&self.evidence_directory, &format!("{name}.request.json"), &response.request_bytes)?;
            retain_original(&self.evidence_directory, &format!("{name}.response.json"), &response.bytes)?;
            retain_original(&self.evidence_directory, &format!("{name}.bracket.json"), &brackets)?;
            ensure!(raw_sha256(&response.bytes) == response.sha256, "clock raw custody mismatch");
            let sample = &response.value.sample;
            ensure!(sample.sample_utc_ms <= 9_007_199_254_740_991 && sample.max_error_us <= self.max_host_error_us
                && serde_json::to_value(&sample.association.owner)? == self.owner
                && serde_json::to_value(&sample.association.selection_ref)? == self.selection
                && serde_json::to_value(&sample.source.assurance_ref)? == self.assurance
                && sample.source.raw_sha256 == self.assurance["sha256"].as_str().unwrap_or(""),
                "calibration source or selection association mismatch");
            let valid_hash = |text: &str| text.len() == 64 && text.bytes()
                .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c));
            ensure!(valid_hash(&sample.source.probe_sha256) && !sample.source.source_pins.is_empty()
                && sample.source.source_pins.values().all(|hash| valid_hash(hash))
                && sample.host_boot_id.len() == 36 && sample.host_boot_id.bytes().enumerate().all(|(i, c)| {
                    if matches!(i, 8 | 13 | 18 | 23) { c == b'-' }
                    else { c.is_ascii_digit() || (b'a'..=b'f').contains(&c) }
                }), "calibration source provenance unavailable");
            let source_identity = serde_json::to_string(&serde_json::json!({"source":sample.source,
                "hostBootId":sample.host_boot_id,"originalCutoff":sample.original_cutoff}))?;
            let mut state = self.state.lock().map_err(|_| anyhow::anyhow!("clock state unavailable"))?;
            ensure!(state.source_identity.as_ref().is_none_or(|original| original == &source_identity),
                "calibration original host source changed");
            let cutoff = self.publication_cutoff.min(parse_selected_utc_ms(&self.original_expires_at)?)
                .min(parse_selected_utc_ms(&sample.original_cutoff)?);
            let error_ms = sample.max_error_us.checked_add(999).context("host error overflow")? / 1000;
            let oldest_lower_ms = sample.sample_utc_ms.checked_sub(error_ms).context("host UTC lower bound unavailable")?;
            let anchor = VerifiedUtcAnchor::from_verified_sample_with_rate(response.sent, response.received,
                sample.sample_utc_ms, error_ms, cutoff, self.rate, self.policy.max_calibration_age_ms)?;
            state.witness = state.witness.intersect(&ElapsedWitness::read(&self.policy.guest_boot_id)?)
                    .map_err(|_| ClockFailure("SELECTED_CLOCK_DISCONTINUITY"))?;
            anchor.upper_now_ms(&state.witness.boot)?;
            // A late refresh cannot revive an already-stale prior mapping, even
            // if no foreground reader happened to observe the gap.
            if let Some(previous) = &state.calibration {
                previous.anchor.upper_now_ms(&state.witness.boot)?;
            }
            ensure!(!self.invalid.load(Ordering::SeqCst), "calibration concurrently invalidated");
            state.source_identity = Some(source_identity);
            state.calibration = Some(Calibration { anchor, oldest_lower_ms });
            Ok(())
        })())
    }
    pub fn start_maintenance(self: &Arc<Self>) -> Result<()> {
        let mut maintenance = self
            .maintenance
            .lock()
            .map_err(|_| anyhow::anyhow!("clock maintenance unavailable"))?;
        ensure!(maintenance.is_none(), "clock maintenance already started");
        let wake = Arc::new((Mutex::new(false), Condvar::new()));
        let stop = wake.clone();
        let clock = self.clone();
        let worker = std::thread::Builder::new()
            .name("trollm-clock-maintenance".into())
            .spawn(move || loop {
                let (lock, event) = &*stop;
                let stopped = lock.lock().unwrap_or_else(|e| e.into_inner());
                if *stopped {
                    break;
                }
                let (stopped, _) = event
                    .wait_timeout(stopped, Duration::from_millis(100))
                    .unwrap_or_else(|e| e.into_inner());
                if *stopped {
                    break;
                }
                drop(stopped);
                if clock.refresh_if_due().is_err() {
                    break;
                }
            })?;
        *maintenance = Some(Maintenance { wake, worker });
        Ok(())
    }
    pub fn stop_maintenance(&self) {
        let maintenance = self
            .maintenance
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        if let Some(maintenance) = maintenance {
            let _ = self
                .failure
                .compare_exchange(0, 5, Ordering::SeqCst, Ordering::SeqCst);
            self.invalid.store(true, Ordering::SeqCst);
            let (lock, event) = &*maintenance.wake;
            *lock.lock().unwrap_or_else(|e| e.into_inner()) = true;
            event.notify_all();
            // Actual inactivity is never published before this owned child exits.
            // In-flight requests retain their original bounded HTTP deadline.
            let _ = maintenance.worker.join();
        }
    }
    fn refresh_if_due(&self) -> Result<()> {
        let due = self.fence((|| {
            ensure!(
                !self.invalid.load(Ordering::SeqCst),
                "calibrated clock permanently fenced"
            );
            let state = self
                .state
                .lock()
                .map_err(|_| anyhow::anyhow!("clock state unavailable"))?;
            let now = ElapsedWitness::read(&self.policy.guest_boot_id)?;
            state
                .witness
                .intersect(&now)
                .map_err(|_| ClockFailure("SELECTED_CLOCK_DISCONTINUITY"))?;
            let calibration = state
                .calibration
                .as_ref()
                .context("calibration unavailable")?;
            calibration.anchor.upper_now_ms(&now.boot)?;
            Ok(calibration.anchor.remaining_age_ms(&now.boot)?
                <= self.policy.max_calibration_age_ms / 2)
        })())?;
        if due {
            self.acquire()?;
        }
        Ok(())
    }
    /// Clock syscalls and try_lock only. No credential, file or network wait.
    pub fn sample(&self) -> Result<(u64, u64)> {
        // Brief state publication contention refuses this read, but is not clock
        // evidence loss. Cooperative control retries; final send still refuses.
        let mut state = self
            .state
            .try_lock()
            .map_err(|_| anyhow::Error::new(ClockReadBusy))?;
        self.fence((|| {
            ensure!(
                !self.invalid.load(Ordering::SeqCst),
                "calibrated clock permanently fenced"
            );
            state.witness = state
                .witness
                .intersect(&ElapsedWitness::read(&self.policy.guest_boot_id)?)?;
            let calibration = state
                .calibration
                .as_ref()
                .context("calibrated UTC not acquired")?;
            Ok((
                calibration.anchor.upper_now_ms(&state.witness.boot)?,
                calibration.oldest_lower_ms,
            ))
        })())
    }
}

#[cfg(test)]
pub(crate) mod controls {
    use super::*;
    use std::io::{BufRead, BufReader, Read};
    use std::net::TcpListener;
    use std::sync::{atomic::AtomicUsize, Arc};
    use trollm_mcp::transport::RootCertificates;
    #[allow(dead_code)]
    mod tls {
        include!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../trollm-mcp/tests/common/tls_fixture.rs"
        ));
    }

    fn clock_fixture(
        mode: &'static str,
    ) -> (
        CalibratedClock,
        std::thread::JoinHandle<()>,
        Arc<AtomicUsize>,
        PathBuf,
    ) {
        clock_fixture_responses(mode, 1)
    }
    pub(crate) fn clock_fixture_responses(
        mode: &'static str,
        responses: usize,
    ) -> (
        CalibratedClock,
        std::thread::JoinHandle<()>,
        Arc<AtomicUsize>,
        PathBuf,
    ) {
        let selection =
            crate::collection::tests::decode(&crate::collection::tests::body()).unwrap();
        clock_fixture_for_selection(mode, responses, selection)
    }
    pub(crate) fn binding_fixture(
        selection: SelectionRecord,
    ) -> (
        PrivateGuardClient,
        CalibratedClockPolicy,
        Value,
        std::thread::JoinHandle<()>,
        Arc<AtomicUsize>,
        PathBuf,
    ) {
        let (clock, server, calls, directory) =
            clock_fixture_for_selection("binding", 3, selection);
        (
            clock.client.clone(),
            clock.policy.clone(),
            clock.assurance.clone(),
            server,
            calls,
            directory,
        )
    }
    fn clock_fixture_for_selection(
        mode: &'static str,
        responses: usize,
        selection: SelectionRecord,
    ) -> (
        CalibratedClock,
        std::thread::JoinHandle<()>,
        Arc<AtomicUsize>,
        PathBuf,
    ) {
        use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
        let boot = BootSample::read().unwrap();
        let expiry = "2099-01-01T00:00:00.000Z";
        let assurance = if mode == "binding" {
            serde_json::json!({"tenantId":selection.selection.owner.tenant_id,
            "artifactId":format!("art_{}","f".repeat(32)),"sha256":"a".repeat(64)})
        } else {
            serde_json::to_value(&selection.selection_ref).unwrap()
        };
        let ready = serde_json::json!({"expiresAt":expiry,"preparationPolicy":{"clockPolicy":{
            "maxClockErrorMs":1000,"assuranceRef":assurance}}});
        let directory = std::env::temp_dir().join(format!(
            "calibrated-clock-{}",
            trollm_core::RunDir::under(Path::new("unused")).run_id()
        ));
        std::fs::create_dir(&directory).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!(
            "https://localhost:{}/guard",
            listener.local_addr().unwrap().port()
        );
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = calls.clone();
        let client = PrivateGuardClient::new(
            &url,
            Arc::new(move || {
                counter.fetch_add(1, Ordering::SeqCst);
                Some("declared-clock-only-token".into())
            }),
            RootCertificates::from_pem(tls::ROOT_1_PEM).unwrap(),
        )
        .unwrap();
        let policy = CalibratedClockPolicy {
            profile_id: "trollm.android-selected-emulator-calibrated-clock.v1-proposal".into(),
            guest_boot_id: boot.boot_id().into(),
            minimum_rate_numerator: if matches!(
                mode,
                "refresh" | "late-refresh" | "source-loss" | "stop-held" | "binding"
            ) {
                9
            } else {
                1
            },
            minimum_rate_denominator: if matches!(
                mode,
                "refresh" | "late-refresh" | "source-loss" | "stop-held" | "binding"
            ) {
                10
            } else {
                2
            },
            max_calibration_age_ms: if mode == "binding" {
                30000
            } else if mode == "late" {
                100
            } else if matches!(
                mode,
                "refresh" | "late-refresh" | "source-loss" | "stop-held"
            ) {
                1000
            } else {
                10000
            },
        }; // Declared test engineering bound, not guest/platform qualification.
        let clock = CalibratedClock::new(
            policy,
            client,
            &ready,
            &selection,
            if mode == "cutoff" {
                (std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_millis() as u64)
                    + 1200
            } else {
                parse_selected_utc_ms(expiry).unwrap()
            },
            directory.clone(),
        )
        .unwrap();
        let owner = serde_json::to_value(&selection.selection.owner).unwrap();
        let reference = serde_json::to_value(&selection.selection_ref).unwrap();
        let expiry = expiry.to_owned();
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
            for response_index in 0..if responses == 0 {
                usize::MAX
            } else {
                responses
            } {
                let deadline =
                    Instant::now() + Duration::from_secs(if responses == 0 { 2 } else { 5 });
                let socket = loop {
                    match listener.accept() {
                        Ok((socket, _)) => break socket,
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            if responses == 0 && Instant::now() >= deadline {
                                return;
                            }
                            assert!(Instant::now() < deadline);
                            std::thread::sleep(Duration::from_millis(2));
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
                let mut reader = BufReader::new(rustls::StreamOwned::new(
                    rustls::ServerConnection::new(Arc::new(config.clone())).unwrap(),
                    socket,
                ));
                let mut first = String::new();
                reader.read_line(&mut first).unwrap();
                let actor_control = mode == "binding" && first.starts_with("GET /control ");
                assert!(actor_control || first.starts_with("POST /control "));
                let mut length = 0usize;
                loop {
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    if line == "\r\n" {
                        break;
                    }
                    if let Some((key, value)) = line.split_once(':') {
                        if key.eq_ignore_ascii_case("content-length") {
                            length = value.trim().parse().unwrap();
                        }
                    }
                }
                assert!(if actor_control {
                    length == 0
                } else {
                    (1..=4096).contains(&length)
                });
                if actor_control {
                    let bytes = serde_json::to_vec(
                        &serde_json::json!({"schema":"trollm.private-guard-control.v1",
                        "invocationKey":"test-invocation","expiresAt":expiry,"aborted":false}),
                    )
                    .unwrap();
                    write!(
                        reader.get_mut(),
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        bytes.len()
                    )
                    .unwrap();
                    reader.get_mut().write_all(&bytes).unwrap();
                    reader.get_mut().flush().unwrap();
                    continue;
                }
                let mut original = vec![0; length];
                reader.read_exact(&mut original).unwrap();
                let request: Value = serde_json::from_slice(&original).unwrap();
                assert_eq!(request["invocationKey"], "test-invocation");
                let sample = serde_json::json!({"sampleUtcMs":std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64,
                "maxErrorUs":1000,"hostBootId":"abcdefab-1234-1234-1234-123456789abc","originalCutoff":expiry,
                "source":{"assuranceRef":assurance,"rawSha256":assurance["sha256"],
                    "probeSha256":"d".repeat(64),"sourcePins":{"declared-source":"e".repeat(64)}},
                "association":{"owner":owner,"selectionRef":reference,"invocationKey":"test-invocation"}});
                let mut value = serde_json::json!({"schema":"trollm.private-guard-clock.v1-proposal",
                "invocationKey":request["invocationKey"],"guestBootId":request["guestBootId"],
                "challenge":request["challenge"],"expiresAt":expiry,"aborted":false,"sample":sample});
                match mode {
                    "challenge" => value["challenge"] = serde_json::json!("0".repeat(64)),
                    "selection" => {
                        value["sample"]["association"]["selectionRef"]["sha256"] =
                            serde_json::json!("0".repeat(64))
                    }
                    "late" => std::thread::sleep(Duration::from_millis(200)),
                    "refresh" if response_index > 0 => {
                        std::thread::sleep(Duration::from_millis(150))
                    }
                    "late-refresh" if response_index > 0 => {
                        std::thread::sleep(Duration::from_millis(700))
                    }
                    "source-loss" if response_index > 0 => {
                        value["challenge"] = serde_json::json!("0".repeat(64))
                    }
                    "stop-held" if response_index > 0 => {
                        std::thread::sleep(Duration::from_millis(200))
                    }
                    _ => {}
                }
                let bytes = serde_json::to_vec(&value).unwrap();
                // Late response may encounter a legitimately closed native request.
                let _ = write!(reader.get_mut(), "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nx-response-sha256: {}\r\nConnection: close\r\n\r\n",
                bytes.len(), raw_sha256(&bytes));
                let _ = reader.get_mut().write_all(&bytes);
                let _ = reader.get_mut().flush();
            }
        });
        (clock, server, calls, directory)
    }
    #[test]
    fn actual_private_tls_calibration_preserves_originals_and_final_read_is_local() {
        let (clock, server, calls, directory) = clock_fixture("positive");
        clock.acquire().unwrap();
        server.join().unwrap();
        let (upper, lower) = clock.sample().unwrap();
        assert!(upper >= lower);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let files: Vec<_> = std::fs::read_dir(&directory)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect();
        assert_eq!(files.len(), 3);
        let bracket: Value = serde_json::from_slice(
            &std::fs::read(directory.join("clock-calibration-000001.bracket.json")).unwrap(),
        )
        .unwrap();
        for (endpoint, original) in [("sentEndpoint", "sent"), ("receivedEndpoint", "received")] {
            assert_eq!(bracket[endpoint]["boot"], bracket[original]);
            let before = bracket[endpoint]["monotonicBeforeNs"].as_u64().unwrap();
            let after = bracket[endpoint]["monotonicAfterNs"].as_u64().unwrap();
            assert!(before > 0 && after >= before);
        }
        assert!(
            bracket["receivedEndpoint"]["monotonicBeforeNs"]
                .as_u64()
                .unwrap()
                >= bracket["sentEndpoint"]["monotonicAfterNs"]
                    .as_u64()
                    .unwrap()
        );

        for file in files {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(file).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        std::fs::remove_dir_all(directory).unwrap();
    }
    #[test]
    fn owned_refresh_spans_multiple_mapping_lifetimes_without_blocking_local_reads() {
        let (clock, server, calls, directory) = clock_fixture_responses("refresh", 3);
        let clock = Arc::new(clock);
        clock.acquire().unwrap();
        clock.start_maintenance().unwrap();
        let started = Instant::now();
        let deadline = started + Duration::from_secs(5);
        loop {
            assert!(Instant::now() < deadline);
            match clock.sample() {
                Ok(_) => {}
                Err(error) => assert!(error.downcast_ref::<ClockReadBusy>().is_some(), "{error:#}"),
            }
            if directory
                .join("clock-calibration-000003.bracket.json")
                .exists()
                && clock.acquisition.try_lock().is_ok()
            {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(started.elapsed() > Duration::from_millis(900));
        clock.sample().unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 3);
        clock.stop_maintenance();
        server.join().unwrap();
        assert!(clock.sample().is_err());
        assert!(clock.maintenance.lock().unwrap().is_none());
        std::fs::remove_dir_all(directory).unwrap();
    }
    #[test]
    fn original_deadline_source_loss_and_late_refresh_permanently_refuse() {
        for (mode, responses, expected) in [
            ("cutoff", 1, "SELECTED_CLOCK_DEADLINE"),
            ("late-refresh", 2, "SELECTED_CLOCK_REFRESH_TIMEOUT"),
            ("source-loss", 2, "SELECTED_CLOCK_SOURCE_UNAVAILABLE"),
        ] {
            let (clock, server, _, directory) = clock_fixture_responses(mode, responses);
            let clock = Arc::new(clock);
            clock.acquire().unwrap();
            clock.start_maintenance().unwrap();
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                assert!(Instant::now() < deadline);
                match clock.sample() {
                    Ok(_) => {}
                    Err(error) if error.downcast_ref::<ClockReadBusy>().is_some() => {}
                    Err(error) => {
                        let actual = error.downcast_ref::<ClockFailure>().unwrap().0;
                        if mode == "late-refresh" {
                            // Either original local-age refusal or the derived
                            // original HTTP deadline can be observed first.
                            assert!(["SELECTED_CLOCK_STALE", "SELECTED_CLOCK_REFRESH_TIMEOUT"]
                                .contains(&actual));
                        } else {
                            assert_eq!(actual, expected);
                        }
                        break;
                    }
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            let original_cause = clock.failure_code();
            clock.stop_maintenance();
            server.join().unwrap();
            assert_eq!(clock.failure_code(), original_cause);
            assert!(clock.acquire().is_err());
            assert!(clock.sample().is_err());
            std::fs::remove_dir_all(directory).unwrap();
        }
    }
    #[test]
    fn stop_joins_original_inflight_refresh_before_returning() {
        let (clock, server, calls, directory) = clock_fixture_responses("stop-held", 2);
        let clock = Arc::new(clock);
        clock.acquire().unwrap();
        clock.start_maintenance().unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        while calls.load(Ordering::SeqCst) < 2 {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(2));
        }
        clock.stop_maintenance();
        assert!(clock.maintenance.lock().unwrap().is_none());
        server.join().unwrap();
        assert_eq!(clock.failure_code(), "SELECTED_CLOCK_RETIRED");
        assert!(clock.sample().is_err());
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn unavailable_mapping_fences_without_network_retry() {
        let (clock, server, calls, directory) = clock_fixture("positive");
        clock.acquire().unwrap();
        server.join().unwrap();
        clock.state.lock().unwrap().calibration = None;
        assert!(clock.sample().is_err());
        assert!(clock.refresh_if_due().is_err());
        assert!(clock.acquire().is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn foreign_response_challenge_selection_and_held_acquisition_permanently_fence() {
        for mode in ["challenge", "selection", "late"] {
            let (clock, server, calls, directory) = clock_fixture(mode);
            assert!(clock.acquire().is_err());
            server.join().unwrap();
            assert!(clock.sample().is_err());
            assert!(clock.acquire().is_err());
            assert_eq!(calls.load(Ordering::SeqCst), 1); // No source/effect retry.
            std::fs::remove_dir_all(directory).unwrap();
        }
    }
    #[test]
    fn missing_or_mixed_platform_policy_never_selects_clock_support() {
        let original = serde_json::json!({"profileId":"trollm.android-selected-emulator-calibrated-clock.v1-proposal",
            "guestBootId":"11111111-2222-4333-8444-555555555555", "minimumRateNumerator":1,
            "minimumRateDenominator":2,"maxCalibrationAgeMs":10000});
        for (field, value) in [
            ("minimumRateNumerator", serde_json::json!(0)),
            ("minimumRateDenominator", serde_json::json!(0)),
            ("maxCalibrationAgeMs", serde_json::json!(0)),
            ("profileId", serde_json::json!("framework-confidence-100")),
        ] {
            let mut changed = original.clone();
            changed[field] = value;
            assert!(serde_json::from_value::<CalibratedClockPolicy>(changed)
                .unwrap()
                .validate()
                .is_err());
        }
        let mut extra = original;
        extra["synchronized"] = serde_json::json!(true);
        assert!(serde_json::from_value::<CalibratedClockPolicy>(extra).is_err());
    }
}
