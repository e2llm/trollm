//! Synchronous read-only private adapter client. It neither grants native
//! admission nor sends Relay effects. The selected provider owns semantics.
use crate::preparation::{
    js_sorted_json_bytes, raw_sha256, PreparationContext, RawContext, MAX_PREPARATION_BYTES,
};
use crate::transport::{required_grant_token, McpClient, RootCertificates, TokenGetter};
use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::Read;
use std::time::{Duration, Instant};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GuardRequest<'a> {
    pub schema: &'static str,
    pub invocation_key: &'a str,
    pub selection_ref: &'a Value,
    pub native_context: RawContext,
    pub guard_request: &'a Value,
}

impl<'a> GuardRequest<'a> {
    pub fn new(
        invocation_key: &'a str,
        selection_ref: &'a Value,
        context: &PreparationContext,
        request: &'a Value,
    ) -> Result<Self> {
        ensure!(
            !invocation_key.is_empty() && invocation_key.len() <= 256,
            "invalid guard invocation"
        );
        ensure!(
            selection_ref.is_object() && request.is_object(),
            "private guard requires objects"
        );
        Ok(Self {
            schema: "trollm.private-collection-guard.v1",
            invocation_key,
            selection_ref,
            native_context: context.raw()?,
            guard_request: request,
        })
    }
}

/// Newly generated adapter bytes. Never described as original Relay bytes.
pub struct GuardResponse {
    pub bytes: Vec<u8>,
    pub sha256: String,
    pub context_sha256: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GuardControl {
    pub schema: String,
    pub invocation_key: String,
    pub aborted: bool,
    pub expires_at: String,
}

/// Exact RPA9c97 private sampler result. It supplies retrospective host UTC
/// observation only; no guest rate, future lease or runtime admission is decoded.
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GuardClockSample {
    pub sample_utc_ms: u64,
    pub max_error_us: u64,
    pub host_boot_id: String,
    pub original_cutoff: String,
    pub source: GuardClockSource,
    pub association: GuardClockAssociation,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GuardClockSource {
    pub assurance_ref: GuardClockRef,
    pub raw_sha256: String,
    pub probe_sha256: String,
    pub source_pins: std::collections::BTreeMap<String, String>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GuardClockRef {
    pub tenant_id: String,
    pub artifact_id: String,
    pub sha256: String,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GuardClockAssociation {
    pub owner: Value,
    pub selection_ref: GuardClockRef,
    pub invocation_key: String,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GuardClockControl {
    pub schema: String,
    pub invocation_key: String,
    pub challenge: String,
    pub guest_boot_id: String,
    pub aborted: bool,
    pub expires_at: String,
    pub sample: GuardClockSample,
}
#[derive(Debug)]
pub struct GuardClockTimeout;
impl std::fmt::Display for GuardClockTimeout {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SELECTED_CLOCK_REFRESH_TIMEOUT")
    }
}
impl std::error::Error for GuardClockTimeout {}

/// Closed acquisition categories. Connect includes TLS/DNS/connect failures;
/// it deliberately does not guess a lower-level cause from an error string.
#[derive(Clone, Copy, Debug)]
pub enum GuardClockFailure {
    Connect,
    Request,
    HttpStatus,
    BodyRead,
    BodyLimit,
    HashMissing,
    HashMismatch,
    ResponseFormat,
    Association,
    EvidenceIo,
    GuestRead,
    ChallengeSource,
    Credentials,
}
impl GuardClockFailure {
    pub fn id(self) -> u8 {
        self as u8 + 7
    }
    pub fn code_from_id(id: u8) -> Option<&'static str> {
        [
            "SELECTED_CLOCK_CONNECT_FAILED",
            "SELECTED_CLOCK_REQUEST_FAILED",
            "SELECTED_CLOCK_HTTP_STATUS",
            "SELECTED_CLOCK_BODY_READ",
            "SELECTED_CLOCK_BODY_LIMIT",
            "SELECTED_CLOCK_HASH_MISSING",
            "SELECTED_CLOCK_HASH_MISMATCH",
            "SELECTED_CLOCK_RESPONSE_FORMAT",
            "SELECTED_CLOCK_RESPONSE_ASSOCIATION",
            "SELECTED_CLOCK_EVIDENCE_IO",
            "SELECTED_CLOCK_GUEST_READ",
            "SELECTED_CLOCK_CHALLENGE_SOURCE",
            "SELECTED_CLOCK_CREDENTIALS",
        ]
        .get(id.checked_sub(7)? as usize)
        .copied()
    }
    pub fn code(self) -> &'static str {
        Self::code_from_id(self.id()).expect("closed clock cause")
    }
}
impl std::fmt::Display for GuardClockFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.code())
    }
}
impl std::error::Error for GuardClockFailure {}

/// Original runtime request bracket and raw transport bytes, never restored as
/// a fresh anchor. Installed composition must validate selected source/owner,
/// rate/lifecycle profile and all original expiry minima before using a sample.
pub struct GuardClockResponse {
    pub request_bytes: Vec<u8>,
    pub sent: crate::clock::BootSample,
    pub received: crate::clock::BootSample,
    pub sent_endpoint: crate::clock::ClockEndpoint,
    pub received_endpoint: crate::clock::ClockEndpoint,
    pub bytes: Vec<u8>,
    pub sha256: String,
    pub value: GuardClockControl,
}

#[derive(Clone)]
pub struct PrivateGuardClient {
    url: reqwest::Url,
    http: reqwest::blocking::Client,
    token: TokenGetter,
}

impl PrivateGuardClient {
    pub fn new(url: &str, token: TokenGetter, roots: RootCertificates) -> Result<Self> {
        McpClient::validate_url(url, false)?;
        let url = reqwest::Url::parse(url)?;
        ensure!(
            url.path() == "/guard" && url.query().is_none() && url.fragment().is_none(),
            "private guard URL must end at /guard"
        );
        Ok(Self {
            url,
            http: roots.blocking_client(Duration::from_secs(30))?,
            token,
        })
    }

    pub fn prepare(
        &self,
        request: &GuardRequest<'_>,
        deadline: Instant,
        before_io: &dyn Fn() -> Result<()>,
    ) -> Result<GuardResponse> {
        // Value serialization uses the fixed ASCII IPC field names and retains
        // the opaque native context string without parsing its u64 values.
        let value = serde_json::to_value(request)?;
        let bytes = js_sorted_json_bytes(&value)?;
        ensure!(
            bytes.len() <= 262_144,
            "private guard request exceeds limit"
        );
        let token = required_grant_token(Some(&self.token))?;
        before_io()?;
        let timeout = deadline
            .checked_duration_since(Instant::now())
            .context("guard deadline expired")?;
        let response = self
            .http
            .post(self.url.clone())
            .bearer_auth(token)
            .header("Content-Type", "application/json")
            .body(bytes)
            .timeout(timeout.min(Duration::from_secs(30)))
            .send()
            .context("private guard read failed")?;
        ensure!(
            response.status().as_u16() == 200,
            "private guard refused request"
        );
        let context = response
            .headers()
            .get("x-native-context-sha256")
            .and_then(|v| v.to_str().ok())
            .context("missing private guard context hash")?
            .to_owned();
        let advertised = response
            .headers()
            .get("x-response-sha256")
            .and_then(|v| v.to_str().ok())
            .context("missing private guard response hash")?
            .to_owned();
        let mut bytes = vec![];
        response
            .take((MAX_PREPARATION_BYTES + 1) as u64)
            .read_to_end(&mut bytes)?;
        ensure!(
            bytes.len() <= MAX_PREPARATION_BYTES,
            "private guard response exceeds limit"
        );
        ensure!(Instant::now() < deadline, "late private guard response");
        before_io()?;
        ensure!(
            context == request.native_context.sha256,
            "private guard context mismatch"
        );
        let sha256 = raw_sha256(&bytes);
        ensure!(sha256 == advertised, "private guard response hash mismatch");
        let value: Value = serde_json::from_slice(&bytes)?;
        ensure!(
            value.is_object(),
            "private guard response must be an object"
        );
        Ok(GuardResponse {
            bytes,
            sha256,
            context_sha256: context,
        })
    }

    /// Report a previously authenticated retained outcome. This operation sends
    /// no Relay effect and never retries automatically. Recovery must reuse the
    /// retained exact request under the same adapter lifetime/original cutoff.
    pub fn acknowledge_outcome(
        &self,
        request: &GuardRequest<'_>,
        deadline: Instant,
        before_io: &dyn Fn() -> Result<()>,
    ) -> Result<GuardResponse> {
        ensure!(
            request.guard_request["kind"] == "outcome"
                && request.guard_request["outcome"] == "OBSERVATION_RETURNED"
                && request.guard_request["contextSha256"] == request.native_context.sha256,
            "native retained outcome association unavailable"
        );
        let response = self.prepare(request, deadline, before_io)?;
        let ack: Value = serde_json::from_slice(&response.bytes)?;
        let fields = ack
            .as_object()
            .context("private outcome ACK object unavailable")?;
        ensure!(
            fields.len() == 2
                && ack["recorded"] == true
                && ack["source"] == "native-adapter-reported-not-independent-settlement",
            "private outcome ACK refused"
        );
        Ok(response)
    }

    /// Passive startup/acquisition control over the existing private installed
    /// adapter. No associated worker, model invocation or Relay operation needed.
    pub fn acquire_clock(
        &self,
        invocation_key: &str,
        guest_boot_id: &str,
        original_expires_at: &str,
        deadline: Instant,
        before_io: &dyn Fn() -> Result<()>,
        retain: &dyn Fn(&str, &[u8]) -> Result<()>,
    ) -> Result<GuardClockResponse> {
        let keep =
            |name: &str, bytes: &[u8]| retain(name, bytes).context(GuardClockFailure::EvidenceIo);
        let sent_endpoint =
            crate::clock::ClockEndpoint::read().context(GuardClockFailure::GuestRead)?;
        keep(
            "sent-bracket.json",
            &serde_json::to_vec(&serde_json::json!({
            "schema":"trollm.clock-calibration-sent.v1-proposal", "sentEndpoint":sent_endpoint,
            "receivedEndpoint":null}))?,
        )?;
        let sent = sent_endpoint.boot.clone();
        ensure!(
            sent.boot_id() == guest_boot_id,
            GuardClockFailure::Association
        );
        before_io()?;
        let mut random = [0u8; 32];
        std::fs::File::open("/dev/urandom")
            .and_then(|mut source| source.read_exact(&mut random))
            .context(GuardClockFailure::ChallengeSource)?;
        let challenge: String = random.iter().map(|byte| format!("{byte:02x}")).collect();
        let request_bytes = js_sorted_json_bytes(&serde_json::json!({
            "schema":"trollm.private-guard-clock-request.v1-proposal",
            "invocationKey":invocation_key,"challenge":challenge,"guestBootId":guest_boot_id
        }))?;
        keep("request.json", &request_bytes)?;
        let token =
            required_grant_token(Some(&self.token)).context(GuardClockFailure::Credentials)?;
        before_io()?;
        let timeout = deadline
            .checked_duration_since(Instant::now())
            .context(GuardClockTimeout)?;
        let mut url = self.url.clone();
        url.set_path("/control");
        let response = self
            .http
            .post(url)
            .bearer_auth(token)
            .header("content-type", "application/json")
            .timeout(timeout.min(Duration::from_secs(30)))
            .body(request_bytes.clone())
            .send()
            .map_err(|error| {
                if error.is_timeout() {
                    anyhow::Error::new(GuardClockTimeout)
                } else if error.is_connect() {
                    anyhow::Error::new(error).context(GuardClockFailure::Connect)
                } else {
                    anyhow::Error::new(error).context(GuardClockFailure::Request)
                }
            })?;
        let status = response.status().as_u16();
        let advertised = response
            .headers()
            .get("x-response-sha256")
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        keep(
            "response-metadata.json",
            &serde_json::to_vec(&serde_json::json!({
            "status":status,"advertisedSha256":advertised}))?,
        )?;
        let mut bytes = Vec::new();
        let body_result = response.take(32769).read_to_end(&mut bytes);
        keep("response.json", &bytes)?;
        keep(
            "body-read.json",
            &serde_json::to_vec(&serde_json::json!({
            "complete":body_result.is_ok() && bytes.len()<=32768,"sizeBytes":bytes.len(),
            "boundedPrefix":bytes.len()>32768}))?,
        )?;
        let received_endpoint = crate::clock::ClockEndpoint::read();
        if let Ok(endpoint) = &received_endpoint {
            keep(
                "received-bracket.json",
                &serde_json::to_vec(&serde_json::json!({
                "receivedEndpoint":endpoint,"responseSha256":raw_sha256(&bytes)}))?,
            )?;
        } else {
            keep(
                "received-unavailable.json",
                &serde_json::to_vec(&serde_json::json!({
                "cause":GuardClockFailure::GuestRead.code(),"receivedEndpoint":null}))?,
            )?;
        }
        body_result.map_err(|error| {
            if error.kind() == std::io::ErrorKind::TimedOut
                || error
                    .get_ref()
                    .and_then(|source| source.downcast_ref::<reqwest::Error>())
                    .is_some_and(reqwest::Error::is_timeout)
            {
                anyhow::Error::new(GuardClockTimeout)
            } else {
                anyhow::Error::new(error).context(GuardClockFailure::BodyRead)
            }
        })?;
        let received_endpoint = received_endpoint.context(GuardClockFailure::GuestRead)?;
        let received = received_endpoint.boot.clone();
        ensure!(bytes.len() <= 32768, GuardClockFailure::BodyLimit);
        ensure!(Instant::now() < deadline, GuardClockTimeout);
        before_io()?;
        ensure!(status == 200, GuardClockFailure::HttpStatus);
        let advertised = advertised.context(GuardClockFailure::HashMissing)?;
        let sha256 = raw_sha256(&bytes);
        ensure!(sha256 == advertised, GuardClockFailure::HashMismatch);
        let value: GuardClockControl =
            serde_json::from_slice(&bytes).context(GuardClockFailure::ResponseFormat)?;
        ensure!(
            value.schema == "trollm.private-guard-clock.v1-proposal"
                && value.invocation_key == invocation_key
                && value.challenge == challenge
                && value.guest_boot_id == guest_boot_id
                && received.boot_id() == guest_boot_id
                && value.expires_at == original_expires_at
                && !value.aborted
                && value.sample.association.invocation_key == invocation_key,
            GuardClockFailure::Association
        );
        Ok(GuardClockResponse {
            request_bytes,
            sent_endpoint,
            received_endpoint,
            sent,
            received,
            bytes,
            sha256,
            value,
        })
    }

    /// This reports adapter abort state, not native worker stop or effect outcome.
    pub fn control(
        &self,
        deadline: Instant,
        before_io: &dyn Fn() -> Result<()>,
    ) -> Result<GuardControl> {
        let token = required_grant_token(Some(&self.token))?;
        before_io()?;
        let timeout = deadline
            .checked_duration_since(Instant::now())
            .context("control deadline expired")?;
        let mut url = self.url.clone();
        url.set_path("/control");
        let response = self
            .http
            .get(url)
            .bearer_auth(token)
            .timeout(timeout.min(Duration::from_secs(30)))
            .send()?;
        ensure!(
            response.status().as_u16() == 200,
            "private guard control refused"
        );
        let mut bytes = vec![];
        response.take(4097).read_to_end(&mut bytes)?;
        ensure!(
            bytes.len() <= 4096 && Instant::now() < deadline,
            "private guard control exceeded bound"
        );
        before_io()?;
        let control: GuardControl = serde_json::from_slice(&bytes)?;
        ensure!(
            control.schema == "trollm.private-guard-control.v1",
            "private guard control schema mismatch"
        );
        Ok(control)
    }
}
