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
    ) -> Result<GuardClockResponse> {
        let sent_endpoint = crate::clock::ClockEndpoint::read()?;
        let sent = sent_endpoint.boot.clone();
        ensure!(
            sent.boot_id() == guest_boot_id,
            "clock acquisition guest boot mismatch"
        );
        before_io()?;
        let mut random = [0u8; 32];
        std::fs::File::open("/dev/urandom")?.read_exact(&mut random)?;
        let challenge: String = random.iter().map(|byte| format!("{byte:02x}")).collect();
        let bytes = js_sorted_json_bytes(&serde_json::json!({
            "schema":"trollm.private-guard-clock-request.v1-proposal",
            "invocationKey":invocation_key,"challenge":challenge,"guestBootId":guest_boot_id
        }))?;
        let token = required_grant_token(Some(&self.token))?;
        before_io()?;
        let timeout = deadline
            .checked_duration_since(Instant::now())
            .context("clock acquisition deadline expired")?;
        let mut url = self.url.clone();
        url.set_path("/control");
        let response = self
            .http
            .post(url)
            .bearer_auth(token)
            .header("content-type", "application/json")
            .timeout(timeout.min(Duration::from_secs(30)))
            .body(bytes.clone())
            .send()
            .map_err(|error| {
                if error.is_timeout() {
                    anyhow::Error::new(GuardClockTimeout)
                } else {
                    error.into()
                }
            })?;
        ensure!(
            response.status().as_u16() == 200,
            "private clock acquisition refused"
        );
        let advertised = response
            .headers()
            .get("x-response-sha256")
            .and_then(|value| value.to_str().ok())
            .context("clock response hash missing")?
            .to_owned();
        let request_bytes = bytes;
        let mut bytes = Vec::new();
        response
            .take(32769)
            .read_to_end(&mut bytes)
            .map_err(|error| {
                if error.kind() == std::io::ErrorKind::TimedOut
                    || error
                        .get_ref()
                        .and_then(|source| source.downcast_ref::<reqwest::Error>())
                        .is_some_and(reqwest::Error::is_timeout)
                {
                    anyhow::Error::new(GuardClockTimeout)
                } else {
                    error.into()
                }
            })?;
        let received_endpoint = crate::clock::ClockEndpoint::read()?;
        let received = received_endpoint.boot.clone();
        ensure!(
            bytes.len() <= 32768 && Instant::now() < deadline,
            "clock acquisition late or oversized"
        );
        before_io()?;
        let sha256 = raw_sha256(&bytes);
        ensure!(sha256 == advertised, "private clock response hash mismatch");
        let value: GuardClockControl = serde_json::from_slice(&bytes)?;
        ensure!(
            value.schema == "trollm.private-guard-clock.v1-proposal"
                && value.invocation_key == invocation_key
                && value.challenge == challenge
                && value.guest_boot_id == guest_boot_id
                && received.boot_id() == guest_boot_id
                && value.expires_at == original_expires_at
                && !value.aborted
                && value.sample.association.invocation_key == invocation_key,
            "private clock response association mismatch"
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
