//! Endpoint-bound connection settings and public-client OAuth for the desktop host.

use crate::vault::{random_array, Vault};
use anyhow::{anyhow, bail, Context, Result};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use reqwest::{
    blocking::{Client, RequestBuilder},
    redirect::Policy,
};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::Read,
    path::Path,
    sync::{Arc, Mutex, MutexGuard},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use trollm_uniffi::{
    canonical_mcp_endpoint, configuration::ResolvedConnection,
    setup::ConnectionAuth as RequiredAuth, ConnectionAuth, TokenProvider,
};
use url::{Host, Url};
use uuid::Uuid;
use zeroize::{Zeroize, Zeroizing};

const MAX_NAME: usize = 128;
const MAX_URL: usize = 4096;
const MAX_TOKEN: usize = 16 * 1024;
const MAX_ENTRIES: usize = 128;
const MAX_OAUTH_JSON: usize = 64 * 1024;
const MAX_PENDING: usize = 16;
const STATE_TTL: Duration = Duration::from_secs(10 * 60);
const OAUTH_REQUEST_TIME: Duration = Duration::from_secs(20);
const REFRESH_SKEW_SECS: u64 = 60;
const MCP_SCOPE: &str = "mcp:e2llm";

#[derive(Clone, Serialize, Deserialize)]
#[serde(transparent)]
struct Secret(String);

impl Drop for Secret {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    url: String,
    credential: Credential,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "lowercase", deny_unknown_fields)]
enum Credential {
    Anonymous,
    Bearer { id: Uuid, token: Secret },
    Oauth(Grant),
    Unavailable,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Grant {
    id: Uuid,
    token_endpoint: String,
    client_id: String,
    access_token: Secret,
    refresh_token: Option<Secret>,
    expiry: Option<Expiry>,
    scopes: Vec<String>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Expiry {
    expires_at: u64,
    refresh_at: u64,
}

impl Expiry {
    fn from_lifetime(lifetime: Option<u64>, now: u64) -> Option<Self> {
        lifetime.map(|seconds| Self {
            expires_at: now.saturating_add(seconds),
            refresh_at: now.saturating_add(seconds - REFRESH_SKEW_SECS.min(seconds / 2)),
        })
    }
}

impl Grant {
    fn fresh(&self, now: u64) -> bool {
        !self.access_token.0.is_empty()
            && self.expiry.as_ref().is_none_or(|expiry| {
                let until = if self.refresh_token.is_some() {
                    expiry.refresh_at
                } else {
                    expiry.expires_at
                };
                until > now
            })
    }

    fn usable(&self, now: u64) -> bool {
        self.fresh(now) || self.refresh_token.is_some()
    }
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum ManualMode {
    Anonymous,
    Bearer,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ConnectionStatus {
    pub(crate) name: String,
    pub(crate) url: String,
    pub(crate) mode: &'static str,
    pub(crate) connected: bool,
    pub(crate) pending: bool,
    pub(crate) status: &'static str,
}

#[derive(Clone, Copy)]
pub(crate) struct PinRequest<'a> {
    pub(crate) name: &'a str,
    pub(crate) default_url: &'a str,
    pub(crate) required_auth: Option<RequiredAuth>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(transparent)]
pub(crate) struct ConnectionPins(BTreeMap<String, ConnectionPin>);

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ConnectionPin {
    url: String,
    authentication: PinAuth,
}

#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "lowercase", deny_unknown_fields)]
enum PinAuth {
    Anonymous,
    Bearer { id: Uuid },
    Oauth { id: Uuid },
}

impl ConnectionPins {
    pub(crate) fn resolved_connections(&self) -> BTreeMap<String, ResolvedConnection> {
        self.0
            .iter()
            .map(|(name, pin)| {
                (
                    name.clone(),
                    ResolvedConnection {
                        url: pin.url.clone(),
                    },
                )
            })
            .collect()
    }

    pub(crate) fn validate_endpoints(
        &self,
        expected: &BTreeMap<String, ResolvedConnection>,
    ) -> Result<()> {
        if !self.0.keys().eq(expected.keys()) {
            bail!("Saved authentication does not match this run's connections.");
        }
        for (name, pin) in &self.0 {
            check_name(name)?;
            if pin.url != canonical_endpoint(&expected[name].url)? {
                bail!("Saved authentication does not match this run's connection endpoint.");
            }
        }
        Ok(())
    }

    fn get(&self, name: &str, canonical_url: &str) -> Option<&ConnectionPin> {
        self.0.get(name).filter(|pin| pin.url == canonical_url)
    }
}

struct Exchange {
    issuer: String,
    issuer_sha256: String,
    redirect_uri: Url,
    client_id: String,
    scopes: Vec<String>,
    token_endpoint: Url,
    verifier: Secret,
}

struct Pending {
    id: Uuid,
    name: String,
    url: String,
    deadline: Instant,
    exchange: Option<Exchange>,
}

type Entries = BTreeMap<String, Entry>;

struct Inner {
    vault: Vault<Entries>,
    pending: BTreeMap<String, Pending>,
}

pub(crate) struct Connections {
    callback_url: Url,
    inner: Mutex<Inner>,
    refresh: Mutex<()>,
}

impl Connections {
    pub(crate) fn open(root: &Path, callback_url: String) -> Result<Arc<Self>> {
        let callback_url = validate_callback(&callback_url)?;
        Ok(Arc::new(Self {
            callback_url,
            inner: Mutex::new(Inner {
                vault: Vault::open(root)?,
                pending: BTreeMap::new(),
            }),
            refresh: Mutex::new(()),
        }))
    }

    fn lock(&self) -> Result<MutexGuard<'_, Inner>> {
        self.inner
            .lock()
            .map_err(|_| anyhow!("Credential storage is unavailable."))
    }

    pub(crate) fn initialized(&self) -> bool {
        // If an internal lock failed, never advertise that replacing the vault is safe.
        self.lock().map_or(true, |inner| inner.vault.initialized())
    }

    pub(crate) fn unlock(&self, passphrase: &str) -> Result<()> {
        self.lock()?.vault.unlock(passphrase)
    }

    pub(crate) fn pinned_provider(
        self: &Arc<Self>,
        pins: ConnectionPins,
    ) -> Box<dyn TokenProvider> {
        Box::new(Provider {
            connections: Arc::clone(self),
            pins,
        })
    }

    pub(crate) fn capture_pins(&self, expected: &[PinRequest<'_>]) -> Result<ConnectionPins> {
        let inner = self.lock()?;
        let entries = inner.vault.entries()?;
        let now = epoch_seconds();
        let pending_at = Instant::now();
        let mut pins = BTreeMap::new();
        for request in expected {
            let name = request.name;
            check_name(name)?;
            if pending_for(&inner, name, pending_at).is_some() {
                bail!("Finish signing in before starting a new run.");
            }
            let (url, authentication) = match entries.get(name) {
                Some(entry) => {
                    let authentication = match &entry.credential {
                        Credential::Anonymous => PinAuth::Anonymous,
                        Credential::Bearer { id, .. } => PinAuth::Bearer { id: *id },
                        Credential::Oauth(grant) if grant.usable(now) => {
                            PinAuth::Oauth { id: grant.id }
                        }
                        _ => bail!("A required connection needs to be configured again."),
                    };
                    (entry.url.clone(), authentication)
                }
                None => (canonical_endpoint(request.default_url)?, PinAuth::Anonymous),
            };
            if request.required_auth.is_some_and(|required| {
                !matches!(
                    (required, authentication),
                    (RequiredAuth::Bearer, PinAuth::Bearer { .. })
                        | (RequiredAuth::Oauth, PinAuth::Oauth { .. })
                )
            }) {
                bail!("Select the authentication method required by this flow.");
            }
            if pins
                .insert(
                    name.to_owned(),
                    ConnectionPin {
                        url,
                        authentication,
                    },
                )
                .is_some()
            {
                bail!("A run contains duplicate connection names.");
            }
        }
        Ok(ConnectionPins(pins))
    }

    pub(crate) fn validate_pins(&self, pins: &ConnectionPins) -> Result<()> {
        let inner = self.lock()?;
        let entries = inner.vault.entries()?;
        let now = epoch_seconds();
        for (name, pin) in &pins.0 {
            if matches!(
                pinned_auth(entries, name, pin, now),
                ConnectionAuth::Unavailable
            ) {
                bail!("A connection used by this run needs its original credentials.");
            }
        }
        Ok(())
    }

    pub(crate) fn list(&self) -> Result<Vec<ConnectionStatus>> {
        let inner = self.lock()?;
        let entries = inner.vault.entries()?;
        let pending_at = Instant::now();
        let names: BTreeSet<_> = entries
            .keys()
            .map(String::as_str)
            .chain(
                inner
                    .pending
                    .values()
                    .filter(|pending| pending.deadline > pending_at)
                    .map(|pending| pending.name.as_str()),
            )
            .collect();
        Ok(names
            .into_iter()
            .map(|name| {
                let entry = entries.get(name);
                let url = entry.map_or("", |entry| entry.url.as_str());
                connection_status(
                    name,
                    url,
                    entry,
                    None,
                    pending_for(&inner, name, pending_at),
                )
            })
            .collect())
    }

    pub(crate) fn status(
        &self,
        name: &str,
        default_url: &str,
        required_auth: Option<RequiredAuth>,
    ) -> Result<ConnectionStatus> {
        let inner = self.lock()?;
        let entry = inner.vault.entries()?.get(name);
        let default_url = if entry.is_none() {
            canonical_endpoint(default_url).unwrap_or_default()
        } else {
            String::new()
        };
        let url = entry.map_or(default_url.as_str(), |entry| entry.url.as_str());
        Ok(connection_status(
            name,
            url,
            entry,
            required_auth,
            pending_for(&inner, name, Instant::now()),
        ))
    }

    pub(crate) fn save_manual(
        &self,
        name: &str,
        url: &str,
        mode: ManualMode,
        token: Option<&str>,
    ) -> Result<ConnectionStatus> {
        check_name(name)?;
        let url = canonical_endpoint(url)?;
        let credential = match (mode, token) {
            (ManualMode::Anonymous, None | Some("")) => Credential::Anonymous,
            (ManualMode::Bearer, Some(token)) if valid_token(token) => Credential::Bearer {
                id: Uuid::new_v4(),
                token: Secret(token.to_owned()),
            },
            (ManualMode::Bearer, _) => bail!("Enter a valid bearer token."),
            (ManualMode::Anonymous, Some(_)) => {
                bail!("Anonymous connections cannot include a bearer token.")
            }
        };
        let entry = Entry { url, credential };
        let mut inner = self.lock()?;
        inner.pending.retain(|_, pending| pending.name != name);
        inner
            .vault
            .update(|entries| save_entry(entries, name, entry.clone()))?;
        Ok(connection_status(
            name,
            &entry.url,
            Some(&entry),
            None,
            None,
        ))
    }

    pub(crate) fn delete(&self, name: &str) -> Result<()> {
        check_name(name)?;
        let mut inner = self.lock()?;
        inner.pending.retain(|_, pending| pending.name != name);
        if inner.vault.entries()?.contains_key(name) {
            inner.vault.update(|entries| {
                entries.remove(name);
                Ok(())
            })?;
        }
        Ok(())
    }

    pub(crate) fn begin_oauth(&self, name: &str, url: &str) -> Result<String> {
        self.begin_oauth_using(name, url, |endpoint, state| {
            self.prepare_oauth(endpoint, state)
        })
    }

    fn begin_oauth_using(
        &self,
        name: &str,
        url: &str,
        prepare: impl FnOnce(&str, &str) -> Result<(Exchange, String)>,
    ) -> Result<String> {
        check_name(name)?;
        let url = canonical_endpoint(url)?;
        let state = URL_SAFE_NO_PAD.encode(random_array::<32>()?);
        let id = Uuid::new_v4();
        {
            let mut inner = self.lock()?;
            inner
                .pending
                .retain(|_, pending| pending.deadline > Instant::now() && pending.name != name);
            if inner.pending.len() >= MAX_PENDING {
                bail!("Too many sign-ins are pending. Try again later.");
            }
            let entries = inner.vault.entries()?;
            if entries.len() >= MAX_ENTRIES && !entries.contains_key(name) {
                bail!("Too many saved connections.");
            }
            if inner.pending.contains_key(&state) {
                bail!("Could not reserve a sign-in state. Try again.");
            }
            // The vault remains the saved connection. This separate generation
            // owns a possible replacement until its token exchange succeeds.
            inner.pending.insert(
                state.clone(),
                Pending {
                    id,
                    name: name.to_owned(),
                    url: url.clone(),
                    deadline: Instant::now() + STATE_TTL,
                    exchange: None,
                },
            );
        }

        let prepared = prepare(&url, &state).and_then(|(exchange, authorization)| {
            let mut inner = self.lock()?;
            let pending = inner
                .pending
                .get_mut(&state)
                .filter(|pending| pending.id == id && pending.deadline > Instant::now())
                .context("This sign-in was replaced or expired.")?;
            pending.exchange = Some(exchange);
            Ok(authorization)
        });

        if prepared.is_err() {
            let mut inner = self.lock()?;
            if inner
                .pending
                .get(&state)
                .is_some_and(|pending| pending.id == id)
            {
                inner.pending.remove(&state);
            }
        }
        prepared
    }

    fn prepare_oauth(&self, endpoint: &str, state: &str) -> Result<(Exchange, String)> {
        let parsed = Url::parse(endpoint).map_err(|_| anyhow!("Invalid MCP endpoint."))?;
        let origin = parsed.origin().ascii_serialization();
        if raw_authority(endpoint) != raw_authority(&origin) {
            bail!("The OAuth provider must use the canonical MCP origin.");
        }
        let client = oauth_client()?;
        let metadata: Metadata =
            oauth_json(client.get(format!("{origin}/.well-known/oauth-authorization-server")))?;
        let (mut authentication, token_endpoint, registration) = metadata.endpoints(&origin)?;
        let (issuer_sha256, redirect_uri) = self.issuer_callback(&metadata.issuer);
        let registration: Registration = oauth_json(client.post(registration).json(&json!({
            "redirect_uris": [redirect_uri.as_str()],
            "token_endpoint_auth_method": "none",
            "grant_types": ["authorization_code", "refresh_token"],
            "response_types": ["code"],
            "client_name": "Trollm Desktop"
        })))?;
        if !valid_token(&registration.client_id)
            || registration
                .token_endpoint_auth_method
                .as_deref()
                .is_some_and(|method| method != "none")
            || registration
                .redirect_uris
                .as_ref()
                .is_some_and(|urls| !urls.iter().any(|url| url == redirect_uri.as_str()))
        {
            bail!("The provider did not register the desktop OAuth client.");
        }
        let mut scopes = vec![MCP_SCOPE.to_owned()];
        if metadata
            .scopes_supported
            .iter()
            .any(|scope| scope == "offline_access")
        {
            scopes.push("offline_access".into());
        }

        let scope = scopes.join(" ");
        let verifier = Secret(URL_SAFE_NO_PAD.encode(random_array::<32>()?));
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.0.as_bytes()));
        const PARAMETERS: [&str; 7] = [
            "response_type",
            "client_id",
            "redirect_uri",
            "state",
            "code_challenge",
            "code_challenge_method",
            "scope",
        ];
        if authentication
            .query_pairs()
            .any(|(key, _)| PARAMETERS.contains(&key.as_ref()))
        {
            bail!("The provider authorization endpoint contains conflicting parameters.");
        }
        authentication.query_pairs_mut().extend_pairs([
            ("response_type", "code"),
            ("client_id", registration.client_id.as_str()),
            ("redirect_uri", redirect_uri.as_str()),
            ("state", state),
            ("code_challenge", challenge.as_str()),
            ("code_challenge_method", "S256"),
            ("scope", scope.as_str()),
        ]);
        Ok((
            Exchange {
                issuer: metadata.issuer,
                issuer_sha256,
                redirect_uri,
                client_id: registration.client_id,
                scopes,
                token_endpoint,
                verifier,
            },
            authentication.into(),
        ))
    }

    fn issuer_callback(&self, issuer: &str) -> (String, Url) {
        let issuer_sha256 = format!("{:x}", Sha256::digest(issuer.as_bytes()));
        let mut redirect_uri = self.callback_url.clone();
        redirect_uri.set_path(&format!("/oauth/callback/{issuer_sha256}"));
        (issuer_sha256, redirect_uri)
    }

    pub(crate) fn finish_oauth(
        &self,
        issuer_sha256: &str,
        state: &str,
        code: &str,
        iss: Option<&str>,
    ) -> Result<()> {
        self.finish_oauth_using(issuer_sha256, state, code, iss, Self::exchange_code)
    }

    fn finish_oauth_using(
        &self,
        issuer_sha256: &str,
        state: &str,
        code: &str,
        iss: Option<&str>,
        exchange_code: impl FnOnce(Uuid, Exchange, &str) -> Result<Grant>,
    ) -> Result<()> {
        if state.len() != 43
            || !state
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        {
            bail!("This sign-in state is invalid or expired.");
        }
        let (id, exchange) = {
            let mut inner = self.lock()?;
            let pending = inner
                .pending
                .get_mut(state)
                .filter(|pending| pending.deadline > Instant::now())
                .context("This sign-in state is invalid or expired.")?;
            // Taking the exchange before the request makes the state single-use,
            // even when the authorization response or token exchange fails.
            let exchange = pending
                .exchange
                .as_ref()
                .context("This sign-in state is invalid or already used.")?;
            if exchange.issuer_sha256 != issuer_sha256
                || iss.is_some_and(|iss| iss != exchange.issuer)
            {
                bail!("The authorization response does not match this sign-in provider.");
            }
            let exchange = pending.exchange.take().expect("pending exchange matched");
            (pending.id, exchange)
        };

        let result = exchange_code(id, exchange, code);
        let mut inner = self.lock()?;
        if inner
            .pending
            .get(state)
            .is_none_or(|pending| pending.id != id)
        {
            bail!("This sign-in was replaced or expired.");
        }
        let pending = inner
            .pending
            .remove(state)
            .expect("pending sign-in generation matched");
        if pending.deadline <= Instant::now() {
            bail!("This sign-in was replaced or expired.");
        }
        let grant = result?;
        inner.vault.update(|entries| {
            save_entry(
                entries,
                &pending.name,
                Entry {
                    url: pending.url,
                    credential: Credential::Oauth(grant),
                },
            )
        })
    }

    fn exchange_code(id: Uuid, exchange: Exchange, code: &str) -> Result<Grant> {
        if !valid_token(code) {
            bail!("The provider did not return a valid authorization code.");
        }
        let client = oauth_client()?;
        let tokens: Tokens = oauth_json(client.post(exchange.token_endpoint.clone()).form(&[
            ("grant_type", "authorization_code"),
            ("client_id", exchange.client_id.as_str()),
            ("redirect_uri", exchange.redirect_uri.as_str()),
            ("code_verifier", exchange.verifier.0.as_str()),
            ("code", code),
        ]))?;
        tokens.validate()?;
        let scopes = tokens.granted_scopes(&exchange.scopes)?;
        Ok(Grant {
            id,
            token_endpoint: exchange.token_endpoint.into(),
            client_id: exchange.client_id,
            access_token: tokens.access_token,
            refresh_token: tokens.refresh_token,
            expiry: Expiry::from_lifetime(tokens.expires_in, epoch_seconds()),
            scopes,
        })
    }

    fn fresh_token(
        &self,
        name: &str,
        url: &str,
        grant_id: Uuid,
        refresh: impl FnOnce(&Grant) -> std::result::Result<Tokens, RefreshError>,
    ) -> Option<String> {
        let grant = {
            let inner = self.lock().ok()?;
            let grant = pinned_grant(inner.vault.entries().ok()?, name, url, grant_id)?;
            if grant.fresh(epoch_seconds()) {
                return Some(grant.access_token.0.clone());
            }
            grant.refresh_token.as_ref()?;
            grant.clone()
        };
        // Rotating refresh tokens require single-flight exchanges. No connection
        // edit lock is held during network IO, and the original grant is rechecked.
        let _refresh = self.refresh.lock().ok()?;
        let grant = {
            let inner = self.lock().ok()?;
            let current = pinned_grant(inner.vault.entries().ok()?, name, url, grant.id)?;
            if current.fresh(epoch_seconds()) {
                return Some(current.access_token.0.clone());
            }
            current.clone()
        };

        let result = refresh(&grant).and_then(|tokens| {
            let scopes = tokens
                .granted_scopes(&grant.scopes)
                .map_err(|_| RefreshError::Reconnect)?;
            Ok((tokens, scopes))
        });
        let mut inner = self.lock().ok()?;
        pinned_grant(inner.vault.entries().ok()?, name, url, grant_id)?;
        match result {
            Ok((tokens, scopes)) => {
                let mut updated = grant;
                updated.scopes = scopes;
                updated.access_token = tokens.access_token;
                updated.refresh_token = tokens.refresh_token.or(updated.refresh_token);
                updated.expiry = Expiry::from_lifetime(tokens.expires_in, epoch_seconds());
                let access = updated.access_token.0.clone();
                inner
                    .vault
                    .update(|entries| {
                        save_entry(
                            entries,
                            name,
                            Entry {
                                url: url.to_owned(),
                                credential: Credential::Oauth(updated),
                            },
                        )
                    })
                    .ok()?;
                Some(access)
            }
            Err(RefreshError::Reconnect) => {
                let _ = inner.vault.update(|entries| {
                    save_entry(
                        entries,
                        name,
                        Entry {
                            url: url.to_owned(),
                            credential: Credential::Unavailable,
                        },
                    )
                });
                None
            }
            Err(RefreshError::Temporary) => None,
        }
    }
}

fn save_entry(entries: &mut Entries, name: &str, entry: Entry) -> Result<()> {
    if entries.len() >= MAX_ENTRIES && !entries.contains_key(name) {
        bail!("Too many saved connections.");
    }
    entries.insert(name.to_owned(), entry);
    Ok(())
}

fn pending_for<'a>(inner: &'a Inner, name: &str, at: Instant) -> Option<&'a Pending> {
    inner
        .pending
        .values()
        .find(|pending| pending.name == name && pending.deadline > at)
}

fn connection_status(
    name: &str,
    url: &str,
    entry: Option<&Entry>,
    required: Option<RequiredAuth>,
    pending: Option<&Pending>,
) -> ConnectionStatus {
    if let Some(pending) = pending {
        return ConnectionStatus {
            name: name.to_owned(),
            url: pending.url.clone(),
            mode: "unavailable",
            connected: false,
            pending: true,
            status: "Sign-in pending.",
        };
    }
    let (mode, mut connected, mut status) = match entry.map(|entry| &entry.credential) {
        Some(Credential::Bearer { .. }) => ("bearer", true, "Manual bearer saved."),
        Some(Credential::Oauth(grant)) if grant.usable(epoch_seconds()) => {
            ("oauth", true, "OAuth account connected.")
        }
        Some(Credential::Oauth(_) | Credential::Unavailable) => (
            "unavailable",
            false,
            "Sign in again or save a manual connection.",
        ),
        Some(Credential::Anonymous) => ("anonymous", true, "No authentication selected."),
        None if url.is_empty() => (
            "unavailable",
            false,
            "A valid HTTPS MCP endpoint is required.",
        ),
        None if required.is_some() => (
            "unavailable",
            false,
            "Save credentials for this connection.",
        ),
        None => ("anonymous", true, "No authentication selected."),
    };
    if connected
        && required.is_some_and(|required| {
            !matches!(
                (required, mode),
                (RequiredAuth::Bearer, "bearer") | (RequiredAuth::Oauth, "oauth")
            )
        })
    {
        connected = false;
        status = "Select the authentication method required by this flow.";
    }
    ConnectionStatus {
        name: name.to_owned(),
        url: url.to_owned(),
        mode,
        connected,
        pending: false,
        status,
    }
}

fn pinned_grant<'a>(entries: &'a Entries, name: &str, url: &str, id: Uuid) -> Option<&'a Grant> {
    let entry = entries.get(name).filter(|entry| entry.url == url)?;
    match &entry.credential {
        Credential::Oauth(grant) if grant.id == id => Some(grant),
        _ => None,
    }
}

fn pinned_auth(entries: &Entries, name: &str, pin: &ConnectionPin, now: u64) -> ConnectionAuth {
    match pin.authentication {
        PinAuth::Anonymous => ConnectionAuth::Anonymous,
        PinAuth::Bearer { id } if pinned_bearer(entries, name, &pin.url, id).is_some() => {
            ConnectionAuth::Bearer
        }
        PinAuth::Oauth { id }
            if pinned_grant(entries, name, &pin.url, id).is_some_and(|grant| grant.usable(now)) =>
        {
            ConnectionAuth::Oauth {
                grant_id: id.to_string(),
            }
        }
        _ => ConnectionAuth::Unavailable,
    }
}

fn pinned_bearer<'a>(entries: &'a Entries, name: &str, url: &str, id: Uuid) -> Option<&'a Secret> {
    let entry = entries.get(name).filter(|entry| entry.url == url)?;
    match &entry.credential {
        Credential::Bearer { id: current, token } if id == *current => Some(token),
        _ => None,
    }
}

struct Provider {
    connections: Arc<Connections>,
    pins: ConnectionPins,
}

impl TokenProvider for Provider {
    fn bearer_token(&self, server_name: String, server_url: String) -> Option<String> {
        let url = canonical_endpoint(&server_url).ok()?;
        let PinAuth::Bearer { id } = self.pins.get(&server_name, &url)?.authentication else {
            return None;
        };
        let inner = self.connections.lock().ok()?;
        let token = pinned_bearer(inner.vault.entries().ok()?, &server_name, &url, id)?;
        Some(token.0.clone())
    }

    fn connection_auth(&self, server_name: String, server_url: String) -> ConnectionAuth {
        let Ok(url) = canonical_endpoint(&server_url) else {
            return ConnectionAuth::Unavailable;
        };
        let Ok(inner) = self.connections.lock() else {
            return ConnectionAuth::Unavailable;
        };
        let Ok(entries) = inner.vault.entries() else {
            return ConnectionAuth::Unavailable;
        };
        self.pins
            .get(&server_name, &url)
            .map_or(ConnectionAuth::Unavailable, |pin| {
                pinned_auth(entries, &server_name, pin, epoch_seconds())
            })
    }

    fn oauth_token(
        &self,
        server_name: String,
        server_url: String,
        grant_id: String,
    ) -> Option<String> {
        let url = canonical_endpoint(&server_url).ok()?;
        let id = grant_id.parse::<Uuid>().ok()?;
        if id.to_string() != grant_id {
            return None;
        }
        if !matches!(self.pins.get(&server_name, &url)?.authentication, PinAuth::Oauth { id: pinned } if pinned == id)
        {
            return None;
        }
        self.connections
            .fresh_token(&server_name, &url, id, refresh_grant)
    }
}

#[derive(Deserialize)]
struct Metadata {
    issuer: String,
    authorization_endpoint: String,
    token_endpoint: String,
    registration_endpoint: String,
    #[serde(default)]
    scopes_supported: Vec<String>,
    code_challenge_methods_supported: Option<Vec<String>>,
}

impl Metadata {
    fn endpoints(&self, origin: &str) -> Result<(Url, Url, Url)> {
        if canonical_endpoint(&self.issuer)? != format!("{origin}/")
            || self
                .code_challenge_methods_supported
                .as_ref()
                .is_some_and(|methods| !methods.iter().any(|method| method == "S256"))
        {
            bail!("The provider issuer or PKCE configuration is unsupported.");
        }
        let validate = |value: &str| -> Result<Url> {
            let canonical = canonical_endpoint(value)?;
            let url =
                Url::parse(&canonical).map_err(|_| anyhow!("Invalid OAuth provider endpoint."))?;
            // Compare the canonical raw authority as well as URL origin. Matching
            // a normalized origin alone can collapse distinct authority syntax.
            if url.origin().ascii_serialization() != origin
                || raw_authority(&canonical) != raw_authority(origin)
            {
                bail!("The OAuth provider endpoints must share the MCP origin.");
            }
            Ok(url)
        };
        Ok((
            validate(&self.authorization_endpoint)?,
            validate(&self.token_endpoint)?,
            validate(&self.registration_endpoint)?,
        ))
    }
}

#[derive(Deserialize)]
struct Registration {
    client_id: String,
    token_endpoint_auth_method: Option<String>,
    redirect_uris: Option<Vec<String>>,
}

#[derive(Deserialize)]
struct Tokens {
    access_token: Secret,
    token_type: String,
    refresh_token: Option<Secret>,
    expires_in: Option<u64>,
    scope: Option<String>,
}

impl Tokens {
    fn validate(&self) -> Result<()> {
        if !self.token_type.eq_ignore_ascii_case("bearer")
            || self.expires_in == Some(0)
            || !valid_token(&self.access_token.0)
            || self
                .refresh_token
                .as_ref()
                .is_some_and(|token| !valid_token(&token.0))
        {
            bail!("The provider did not return a usable bearer token.");
        }
        Ok(())
    }

    fn granted_scopes(&self, previous: &[String]) -> Result<Vec<String>> {
        let scopes = match self.scope.as_deref() {
            Some(scope) if scope.len() <= MAX_URL => scope.split(' ').map(str::to_owned).collect(),
            Some(_) => bail!("The provider returned too many OAuth scopes."),
            None => previous.to_owned(),
        };
        if !scopes.iter().all(|scope| valid_token(scope))
            || !scopes.iter().any(|scope| scope == MCP_SCOPE)
        {
            bail!("The provider did not grant the required MCP scope.");
        }
        Ok(scopes)
    }
}

enum RefreshError {
    Reconnect,
    Temporary,
}

fn refresh_grant(grant: &Grant) -> std::result::Result<Tokens, RefreshError> {
    let token = grant
        .refresh_token
        .as_ref()
        .ok_or(RefreshError::Reconnect)?;
    let client = oauth_client().map_err(|_| RefreshError::Temporary)?;
    let request = client.post(grant.token_endpoint.clone()).form(&[
        ("grant_type", "refresh_token"),
        ("client_id", grant.client_id.as_str()),
        ("refresh_token", token.0.as_str()),
    ]);
    let (status, body) = oauth_response(request).map_err(|_| RefreshError::Temporary)?;
    decode_refresh(status, &body)
}

fn decode_refresh(
    status: reqwest::StatusCode,
    body: &[u8],
) -> std::result::Result<Tokens, RefreshError> {
    if !status.is_success() {
        #[derive(Deserialize)]
        struct ErrorResponse {
            error: String,
        }
        let invalid_grant = matches!(status.as_u16(), 400 | 401)
            && serde_json::from_slice::<ErrorResponse>(body).is_ok_and(|body| {
                matches!(body.error.as_str(), "invalid_grant" | "invalid_client")
            });
        return Err(if invalid_grant {
            RefreshError::Reconnect
        } else {
            RefreshError::Temporary
        });
    }
    let tokens: Tokens = serde_json::from_slice(body).map_err(|_| RefreshError::Temporary)?;
    tokens.validate().map_err(|_| RefreshError::Temporary)?;
    Ok(tokens)
}

fn oauth_client() -> Result<Client> {
    Client::builder()
        .https_only(true)
        .redirect(Policy::none())
        .retry(reqwest::retry::never())
        .connect_timeout(Duration::from_secs(10))
        .timeout(OAUTH_REQUEST_TIME)
        .build()
        .map_err(|_| anyhow!("Could not initialize the OAuth client."))
}

fn oauth_response(request: RequestBuilder) -> Result<(reqwest::StatusCode, Zeroizing<Vec<u8>>)> {
    let deadline = Instant::now() + OAUTH_REQUEST_TIME;
    let response = request
        .header("Accept", "application/json")
        .send()
        .map_err(|_| {
            anyhow!("Could not reach the OAuth provider over HTTPS with public certificates.")
        })?;
    let status = response.status();
    let mut response = response.take((MAX_OAUTH_JSON + 1) as u64);
    let mut body = Zeroizing::new(Vec::new());
    let mut buffer = Zeroizing::new([0; 4096]);
    loop {
        if Instant::now() >= deadline {
            bail!("The OAuth provider response timed out.");
        }
        let count = response
            .read(&mut *buffer)
            .map_err(|_| anyhow!("Could not read the OAuth provider response."))?;
        if count == 0 {
            break;
        }
        if body.len() + count > MAX_OAUTH_JSON {
            bail!("The OAuth provider response is too large.");
        }
        body.extend_from_slice(&buffer[..count]);
    }
    Ok((status, body))
}

fn oauth_json<T: DeserializeOwned>(request: RequestBuilder) -> Result<T> {
    let (status, body) = oauth_response(request)?;
    if !status.is_success() {
        bail!("The OAuth provider rejected the request.");
    }
    serde_json::from_slice(&body).map_err(|_| anyhow!("The OAuth provider response is invalid."))
}

fn valid_token(token: &str) -> bool {
    !token.is_empty()
        && token.len() <= MAX_TOKEN
        && token.bytes().all(|byte| byte.is_ascii_graphic())
}

fn check_name(name: &str) -> Result<()> {
    if name.is_empty()
        || name.len() > MAX_NAME
        || name.chars().any(|c| c.is_whitespace() || c.is_control())
        || matches!(name, "local" | "flow")
    {
        bail!("Enter a valid MCP connection name.");
    }
    Ok(())
}

fn epoch_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn validate_callback(value: &str) -> Result<Url> {
    let url = Url::parse(value).map_err(|_| anyhow!("Invalid desktop OAuth callback URI."))?;
    let loopback = match url.host() {
        Some(Host::Domain(domain)) => domain == "localhost",
        Some(Host::Ipv4(address)) => address.is_loopback(),
        Some(Host::Ipv6(address)) => address.is_loopback(),
        None => false,
    };
    if url.host().is_none()
        || !(url.scheme() == "https" || url.scheme() == "http" && loopback)
        || !url.username().is_empty()
        || url.password().is_some()
        || url.path() != "/oauth/callback"
        || url.query().is_some()
        || url.fragment().is_some()
    {
        bail!("Invalid desktop OAuth callback URI.");
    }
    Ok(url)
}

fn canonical_endpoint(value: &str) -> Result<String> {
    if value.len() > MAX_URL {
        bail!("The MCP endpoint URL is too long.");
    }
    canonical_mcp_endpoint(value)
        .map_err(|_| anyhow!("Enter a valid HTTPS MCP endpoint without credentials or a fragment."))
}

fn raw_authority(https_url: &str) -> &str {
    https_url
        .strip_prefix("https://")
        .unwrap_or_default()
        .split('/')
        .next()
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vault::private_tempdir;
    use tempfile::TempDir;

    const CALLBACK: &str = "http://127.0.0.1:8080/oauth/callback";
    const ENDPOINT: &str = "https://example.org/mcp?account=one";

    fn connections() -> (TempDir, Arc<Connections>) {
        let root = private_tempdir();
        let connections = Connections::open(root.path(), CALLBACK.into()).unwrap();
        connections.unlock("a test passphrase").unwrap();
        (root, connections)
    }

    fn save_grant(connections: &Connections, grant: Grant) {
        connections
            .lock()
            .unwrap()
            .vault
            .update(|entries| {
                save_entry(
                    entries,
                    "browser",
                    Entry {
                        url: ENDPOINT.into(),
                        credential: Credential::Oauth(grant),
                    },
                )
            })
            .unwrap();
    }

    fn expired_grant() -> Grant {
        Grant {
            id: Uuid::new_v4(),
            token_endpoint: "https://example.org/token".into(),
            client_id: "test-client-id".into(),
            access_token: Secret("original-access-secret".into()),
            refresh_token: Some(Secret("original-refresh-secret".into())),
            expiry: Expiry::from_lifetime(Some(0), 0),
            scopes: vec![MCP_SCOPE.into()],
        }
    }

    fn refreshed() -> Tokens {
        Tokens {
            access_token: Secret("updated-access-secret".into()),
            token_type: "Bearer".into(),
            refresh_token: Some(Secret("rotated-refresh-secret".into())),
            expires_in: Some(3600),
            scope: None,
        }
    }

    #[test]
    fn bearer_requires_the_exact_endpoint_and_never_appears_in_status() {
        let (_root, connections) = connections();
        let fallback = connections.status("browser", ENDPOINT, None).unwrap();
        assert!(fallback.connected);
        assert_eq!(fallback.mode, "anonymous");
        assert!(
            !connections
                .status("browser", ENDPOINT, Some(RequiredAuth::Bearer))
                .unwrap()
                .connected
        );

        let saved = connections
            .save_manual(
                "browser",
                "HTTPS://Example.ORG:443/mcp?account=one",
                ManualMode::Bearer,
                Some("test-bearer-secret"),
            )
            .unwrap();
        let pins = connections
            .capture_pins(&[PinRequest {
                name: "browser",
                default_url: ENDPOINT,
                required_auth: Some(RequiredAuth::Bearer),
            }])
            .unwrap();
        let provider = connections.pinned_provider(pins);
        assert_eq!(saved.url, ENDPOINT);
        assert_eq!(
            provider
                .bearer_token("browser".into(), ENDPOINT.into())
                .as_deref(),
            Some("test-bearer-secret")
        );
        for wrong in [
            "https://example.org/mcp?account=two",
            "https://example.org/other/../mcp?account=one",
            "https://example.org/mcp?account=%6Fne",
        ] {
            assert!(provider
                .bearer_token("browser".into(), wrong.into())
                .is_none());
            assert_eq!(
                provider.connection_auth("browser".into(), wrong.into()),
                ConnectionAuth::Unavailable
            );
        }
        assert!(!serde_json::to_string(&saved)
            .unwrap()
            .contains("test-bearer-secret"));
        assert!(!serde_json::to_string(&connections.list().unwrap())
            .unwrap()
            .contains("test-bearer-secret"));

        connections.delete("browser").unwrap();
        assert!(provider
            .bearer_token("browser".into(), ENDPOINT.into())
            .is_none());
    }

    #[test]
    fn run_pins_keep_manual_generation_and_never_gain_credentials() {
        let (_root, connections) = connections();
        let required = [PinRequest {
            name: "browser",
            default_url: ENDPOINT,
            required_auth: Some(RequiredAuth::Bearer),
        }];
        assert!(connections.capture_pins(&required).is_err());

        let anonymous = connections
            .capture_pins(&[PinRequest {
                required_auth: None,
                ..required[0]
            }])
            .unwrap();
        let anonymous_provider = connections.pinned_provider(anonymous);
        connections
            .save_manual(
                "browser",
                ENDPOINT,
                ManualMode::Bearer,
                Some("first-secret"),
            )
            .unwrap();
        let pins = connections.capture_pins(&required).unwrap();
        let metadata = serde_json::to_value(&pins).unwrap();
        assert!(!metadata.to_string().contains("first-secret"));
        let pins: ConnectionPins = serde_json::from_value(metadata).unwrap();
        assert_eq!(pins.resolved_connections()["browser"].url, ENDPOINT);
        assert!(pins
            .validate_endpoints(&pins.resolved_connections())
            .is_ok());
        assert!(pins.validate_endpoints(&BTreeMap::new()).is_err());
        assert!(connections.validate_pins(&pins).is_ok());
        let provider = connections.pinned_provider(pins.clone());
        assert_eq!(
            provider
                .bearer_token("browser".into(), ENDPOINT.into())
                .as_deref(),
            Some("first-secret")
        );

        assert_eq!(
            anonymous_provider.connection_auth("browser".into(), ENDPOINT.into()),
            ConnectionAuth::Anonymous
        );
        assert!(anonymous_provider
            .bearer_token("browser".into(), ENDPOINT.into())
            .is_none());
        connections
            .save_manual(
                "browser",
                ENDPOINT,
                ManualMode::Bearer,
                Some("replacement-secret"),
            )
            .unwrap();
        assert!(connections.validate_pins(&pins).is_err());
        assert_eq!(
            provider.connection_auth("browser".into(), ENDPOINT.into()),
            ConnectionAuth::Unavailable
        );
        assert!(provider
            .bearer_token("browser".into(), ENDPOINT.into())
            .is_none());
        connections.delete("browser").unwrap();
        assert!(connections.validate_pins(&pins).is_err());
    }

    #[test]
    fn run_pins_refuse_a_replacement_oauth_grant() {
        let (_root, connections) = connections();
        let mut grant = expired_grant();
        let original = grant.id;
        grant.expiry = None;
        save_grant(&connections, grant);
        let pins = connections
            .capture_pins(&[PinRequest {
                name: "browser",
                default_url: "https://unused.example.org/mcp",
                required_auth: Some(RequiredAuth::Oauth),
            }])
            .unwrap();
        assert_eq!(pins.resolved_connections()["browser"].url, ENDPOINT);
        let provider = connections.pinned_provider(pins.clone());
        assert_eq!(
            provider
                .oauth_token("browser".into(), ENDPOINT.into(), original.to_string())
                .as_deref(),
            Some("original-access-secret")
        );
        let mut grant = expired_grant();
        let replacement = grant.id;
        grant.expiry = None;
        save_grant(&connections, grant);
        assert!(connections.validate_pins(&pins).is_err());
        for id in [original, replacement] {
            assert!(provider
                .oauth_token("browser".into(), ENDPOINT.into(), id.to_string())
                .is_none());
        }
    }

    #[test]
    fn oauth_does_not_accept_explicit_scope_reduction() {
        let (_root, connections) = connections();
        let grant = expired_grant();
        let id = grant.id;
        let fallback = [MCP_SCOPE.to_owned(), "offline_access".to_owned()];
        let mut response = refreshed();
        assert_eq!(response.granted_scopes(&fallback).unwrap(), fallback);
        response.scope = Some(MCP_SCOPE.into());
        assert_eq!(response.granted_scopes(&fallback).unwrap(), [MCP_SCOPE]);
        response.scope = Some("offline_access".into());
        assert!(response.granted_scopes(&fallback).is_err());

        save_grant(&connections, grant);
        assert!(connections
            .fresh_token("browser", ENDPOINT, id, |_| Ok(response))
            .is_none());
        assert!(
            !connections
                .status("browser", ENDPOINT, Some(RequiredAuth::Oauth))
                .unwrap()
                .connected
        );
    }

    #[test]
    fn short_lived_tokens_refresh_halfway_without_discarding_valid_nonrefreshable_tokens() {
        let mut grant = expired_grant();
        grant.expiry = Expiry::from_lifetime(Some(10), 100);
        assert!(grant.fresh(104));
        assert!(!grant.fresh(105));
        assert!(grant.usable(105));
        grant.refresh_token = None;
        assert!(grant.fresh(109));
        assert!(!grant.usable(110));
    }

    #[test]
    fn oauth_refresh_rotates_only_the_original_grant() {
        let (_root, connections) = connections();
        let original = expired_grant();
        let id = original.id;
        save_grant(&connections, original);
        assert!(connections
            .fresh_token("browser", ENDPOINT, Uuid::new_v4(), |_| panic!(
                "wrong grant reached refresh"
            ))
            .is_none());
        assert!(connections
            .fresh_token("browser", "https://example.org/other", id, |_| panic!(
                "wrong endpoint reached refresh"
            ))
            .is_none());

        let access = connections.fresh_token("browser", ENDPOINT, id, |grant| {
            assert_eq!(grant.id, id);
            assert_eq!(grant.client_id, "test-client-id");
            assert_eq!(grant.token_endpoint, "https://example.org/token");
            assert_eq!(
                grant.refresh_token.as_ref().unwrap().0,
                "original-refresh-secret"
            );
            Ok(refreshed())
        });
        assert_eq!(access.as_deref(), Some("updated-access-secret"));
        let pins = connections
            .capture_pins(&[PinRequest {
                name: "browser",
                default_url: ENDPOINT,
                required_auth: Some(RequiredAuth::Oauth),
            }])
            .unwrap();
        assert_eq!(
            connections
                .pinned_provider(pins)
                .connection_auth("browser".into(), ENDPOINT.into()),
            ConnectionAuth::Oauth {
                grant_id: id.to_string()
            }
        );
        let inner = connections.lock().unwrap();
        let entry = pinned_grant(inner.vault.entries().unwrap(), "browser", ENDPOINT, id).unwrap();
        assert_eq!(
            entry.refresh_token.as_ref().unwrap().0,
            "rotated-refresh-secret"
        );
    }

    #[test]
    fn stale_refresh_cannot_publish_or_return_tokens_after_replacement() {
        let (_root, connections) = connections();
        let old = expired_grant();
        let old_id = old.id;
        save_grant(&connections, old);
        let replacement = expired_grant();
        let replacement_id = replacement.id;
        let access = connections.fresh_token("browser", ENDPOINT, old_id, |_| {
            // A sign-in or connection edit occurred while the original grant
            // was waiting for its provider to issue refreshed credentials.
            save_grant(&connections, replacement);
            Ok(refreshed())
        });
        assert!(access.is_none());
        let inner = connections.lock().unwrap();
        let entries = inner.vault.entries().unwrap();
        assert!(pinned_grant(entries, "browser", ENDPOINT, old_id).is_none());
        let grant = pinned_grant(entries, "browser", ENDPOINT, replacement_id).unwrap();
        assert_eq!(grant.access_token.0, "original-access-secret");
    }

    fn begin_test_sign_in(connections: &Connections, endpoint: &str) -> (String, String) {
        let issuer = "https://example.org";
        let (issuer_sha256, redirect_uri) = connections.issuer_callback(issuer);
        let state = connections
            .begin_oauth_using("browser", endpoint, |_, state| {
                Ok((
                    Exchange {
                        issuer: issuer.into(),
                        issuer_sha256: issuer_sha256.clone(),
                        redirect_uri,
                        client_id: "test-client".into(),
                        scopes: vec![MCP_SCOPE.into()],
                        token_endpoint: Url::parse("https://example.org/token").unwrap(),
                        verifier: Secret("test-verifier".into()),
                    },
                    state.into(),
                ))
            })
            .unwrap();
        (state, issuer_sha256)
    }

    fn saved_original_grant(connections: &Connections) -> (ConnectionPins, Uuid) {
        let mut grant = expired_grant();
        grant.expiry = None;
        let id = grant.id;
        save_grant(connections, grant);
        let pins = connections.capture_pins(&[browser_oauth()]).unwrap();
        (pins, id)
    }

    fn browser_oauth() -> PinRequest<'static> {
        PinRequest {
            name: "browser",
            default_url: ENDPOINT,
            required_auth: Some(RequiredAuth::Oauth),
        }
    }

    fn new_grant(id: Uuid) -> Grant {
        Grant {
            id,
            access_token: Secret("new-access-secret".into()),
            expiry: None,
            ..expired_grant()
        }
    }

    #[test]
    fn failed_discovery_and_failed_exchange_keep_the_saved_grant() {
        let (_root, connections) = connections();
        let (pins, id) = saved_original_grant(&connections);
        let provider = connections.pinned_provider(pins);

        assert!(connections
            .begin_oauth_using("browser", ENDPOINT, |_, _| {
                bail!("fixture discovery failure")
            })
            .is_err());
        assert!(connections.capture_pins(&[browser_oauth()]).is_ok());

        let (state, issuer) = begin_test_sign_in(&connections, ENDPOINT);
        assert!(connections
            .finish_oauth_using(&issuer, &state, "code", None, |_, _, _| {
                bail!("fixture code exchange failure")
            })
            .is_err());
        let status = connections
            .status("browser", ENDPOINT, Some(RequiredAuth::Oauth))
            .unwrap();
        assert!(status.connected);
        assert_eq!(
            provider
                .oauth_token("browser".into(), ENDPOINT.into(), id.to_string())
                .as_deref(),
            Some("original-access-secret")
        );
    }

    #[test]
    fn pending_or_abandoned_relogin_blocks_new_pins_and_keeps_original_run_usable() {
        let (_root, connections) = connections();
        let (pins, id) = saved_original_grant(&connections);
        let provider = connections.pinned_provider(pins.clone());
        let new_endpoint = "https://example.org/another";
        let (state, _) = begin_test_sign_in(&connections, new_endpoint);
        let pending = connections.list().unwrap().remove(0);
        assert_eq!(pending.url, new_endpoint);
        assert_eq!(pending.status, "Sign-in pending.");
        assert!(!pending.connected);
        assert!(pending.pending);
        assert!(connections.capture_pins(&[browser_oauth()]).is_err());
        assert!(connections.validate_pins(&pins).is_ok());
        assert_eq!(
            provider
                .oauth_token("browser".into(), ENDPOINT.into(), id.to_string())
                .as_deref(),
            Some("original-access-secret")
        );

        connections
            .lock()
            .unwrap()
            .pending
            .get_mut(&state)
            .unwrap()
            .deadline = Instant::now();
        let after = connections.list().unwrap().remove(0);
        assert!(after.connected);
        assert!(!after.pending);
        assert_eq!(after.url, ENDPOINT);
        assert_eq!(
            connections
                .capture_pins(&[browser_oauth()])
                .unwrap()
                .resolved_connections()["browser"]
                .url,
            ENDPOINT
        );
        assert!(connections.validate_pins(&pins).is_ok());
    }

    #[test]
    fn a_new_oauth_attempt_supersedes_the_old_callback_without_revoking_the_saved_grant() {
        let (_root, connections) = connections();
        let (pins, old_id) = saved_original_grant(&connections);
        let (old_state, issuer) = begin_test_sign_in(&connections, ENDPOINT);
        let (latest, _) = begin_test_sign_in(&connections, ENDPOINT);
        assert!(connections
            .finish_oauth_using(&issuer, &old_state, "code", None, |_, _, _| {
                panic!("superseded sign-in should never reach the provider")
            })
            .is_err());
        assert!(connections.validate_pins(&pins).is_ok());

        connections
            .finish_oauth_using(&issuer, &latest, "code", None, |id, _, _| Ok(new_grant(id)))
            .unwrap();
        assert!(connections.validate_pins(&pins).is_err());
        let active = connections.capture_pins(&[browser_oauth()]).unwrap();
        let provider = connections.pinned_provider(active);
        let ConnectionAuth::Oauth { grant_id } =
            provider.connection_auth("browser".into(), ENDPOINT.into())
        else {
            panic!("latest grant was not promoted");
        };
        assert_ne!(grant_id, old_id.to_string());
        assert_eq!(
            provider
                .oauth_token("browser".into(), ENDPOINT.into(), grant_id)
                .as_deref(),
            Some("new-access-secret")
        );
    }

    #[test]
    fn editing_or_deleting_while_an_exchange_is_in_flight_prevents_promotion() {
        for delete in [false, true] {
            let (_root, connections) = connections();
            let (pins, _) = saved_original_grant(&connections);
            let (state, issuer) = begin_test_sign_in(&connections, ENDPOINT);
            let result =
                connections.finish_oauth_using(&issuer, &state, "code", None, |id, _, _| {
                    if delete {
                        connections.delete("browser").unwrap();
                    } else {
                        connections
                            .save_manual(
                                "browser",
                                ENDPOINT,
                                ManualMode::Bearer,
                                Some("edited-token"),
                            )
                            .unwrap();
                    }
                    Ok(new_grant(id))
                });
            assert!(result.unwrap_err().to_string().contains("replaced"));
            assert!(connections.capture_pins(&[browser_oauth()]).is_err());
            assert!(connections.validate_pins(&pins).is_err());
            let list = connections.list().unwrap();
            if delete {
                assert!(list.is_empty());
            } else {
                assert_eq!(list[0].mode, "bearer");
            }
        }
    }

    #[test]
    fn an_invalid_callback_consumes_its_state_without_network_io() {
        let (_root, connections) = connections();
        let state = URL_SAFE_NO_PAD.encode([5; 32]);
        let issuer = "https://example.org";
        let (issuer_sha256, redirect_uri) = connections.issuer_callback(issuer);
        let (other_issuer_sha256, _) = connections.issuer_callback("https://other.example.org");
        assert_eq!(redirect_uri.as_str(), format!("{CALLBACK}/{issuer_sha256}"));
        assert_eq!(issuer_sha256.len(), 64);
        assert_ne!(issuer_sha256, other_issuer_sha256);
        connections.lock().unwrap().pending.insert(
            state.clone(),
            Pending {
                id: Uuid::new_v4(),
                name: "browser".into(),
                url: ENDPOINT.into(),
                deadline: Instant::now() + STATE_TTL,
                exchange: Some(Exchange {
                    issuer: issuer.into(),
                    issuer_sha256: issuer_sha256.clone(),
                    redirect_uri,
                    client_id: "test-client".into(),
                    scopes: vec![MCP_SCOPE.into()],
                    token_endpoint: Url::parse("https://example.org/token").unwrap(),
                    verifier: Secret("test-verifier".into()),
                }),
            },
        );
        assert!(connections
            .finish_oauth(&other_issuer_sha256, &state, "example-code", None)
            .unwrap_err()
            .to_string()
            .contains("provider"));
        assert!(connections
            .finish_oauth(
                &issuer_sha256,
                &state,
                "example-code",
                Some("https://example.org/")
            )
            .unwrap_err()
            .to_string()
            .contains("provider"));
        assert!(connections
            .finish_oauth(&issuer_sha256, &state, "", Some(issuer))
            .unwrap_err()
            .to_string()
            .contains("authorization code"));
        assert!(connections
            .finish_oauth(&issuer_sha256, &state, "any-test-code", None)
            .unwrap_err()
            .to_string()
            .contains("state"));
    }

    #[test]
    fn oauth_discovery_rejects_cross_origin_endpoints_and_invalid_issuers() {
        let origin = "https://example.org";
        let mut metadata = Metadata {
            issuer: origin.into(),
            authorization_endpoint: "https://example.org/authorization".into(),
            token_endpoint: "https://example.org/token".into(),
            registration_endpoint: "https://example.org/registration".into(),
            scopes_supported: vec![],
            code_challenge_methods_supported: Some(vec!["S256".into()]),
        };
        assert!(metadata.endpoints(origin).is_ok());
        metadata.token_endpoint = "https://other.example.org/token".into();
        assert!(metadata.endpoints(origin).is_err());
        metadata.token_endpoint = "https://example.org/token".into();
        metadata.issuer = "https://example.org/other".into();
        assert!(metadata.endpoints(origin).is_err());
        metadata.issuer = origin.into();
        metadata.code_challenge_methods_supported = Some(vec!["plain".into()]);
        assert!(metadata.endpoints(origin).is_err());

        assert!(validate_callback(CALLBACK).is_ok());
        assert!(validate_callback("http://example.org/oauth/callback").is_err());
        assert!(validate_callback("http://127.0.0.1:8080/oauth/callback?other=yes").is_err());
    }

    #[test]
    fn refresh_only_marks_known_provider_rejections_as_reconnect() {
        assert!(matches!(
            decode_refresh(
                reqwest::StatusCode::BAD_REQUEST,
                br#"{"error":"invalid_grant","error_description":"ignored"}"#
            ),
            Err(RefreshError::Reconnect)
        ));
        assert!(matches!(
            decode_refresh(
                reqwest::StatusCode::SERVICE_UNAVAILABLE,
                br#"{"error":"invalid_grant"}"#
            ),
            Err(RefreshError::Temporary)
        ));
        assert!(matches!(
            decode_refresh(
                reqwest::StatusCode::OK,
                br#"{"token_type":"Bearer","access_token":"test-invalid token"}"#
            ),
            Err(RefreshError::Temporary)
        ));
    }
}
