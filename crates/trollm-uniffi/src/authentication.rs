//! Run-local authentication identities. Tokens stay in the app's encrypted store.

use crate::SharedTokenProvider;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::sync::Arc;
use trollm_mcp::ServerConfig;

const AUTH_FILE: &str = "connection-auth.json";

/// App-selected authentication for one endpoint. This contains no credentials.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, uniffi::Enum)]
#[serde(tag = "mode", rename_all = "camelCase", deny_unknown_fields)]
pub enum ConnectionAuth {
    Anonymous,
    Bearer,
    Oauth {
        grant_id: String,
    },
    /// Authentication needs sign-in or explicit endpoint-bound manual setup.
    Unavailable,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PinnedConnection {
    url: String,
    auth: ConnectionAuth,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RunAuthentication {
    version: u32,
    servers: BTreeMap<String, PinnedConnection>,
}

fn endpoint(value: &str) -> Result<String> {
    // Url validates the network destination, but its serialization rewrites dot
    // segments, IP spellings and Unicode. Those are distinct credential pins in
    // Android's URI-based identity; do not feed rewritten values to callbacks.
    if value.chars().any(|c| {
        c.is_whitespace()
            || c.is_control()
            || matches!(c, '\\' | '"' | '<' | '>' | '`' | '^' | '{' | '}' | '|')
    }) {
        bail!("invalid MCP endpoint characters");
    }
    let bytes = value.as_bytes();
    for (index, byte) in bytes.iter().enumerate() {
        if *byte == b'%'
            && !(bytes.get(index + 1).is_some_and(u8::is_ascii_hexdigit)
                && bytes.get(index + 2).is_some_and(u8::is_ascii_hexdigit))
        {
            bail!("invalid MCP endpoint escape");
        }
    }
    let parsed = url::Url::parse(value).context("invalid MCP endpoint")?;
    if parsed.scheme() != "https"
        || parsed.host_str().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.fragment().is_some()
    {
        bail!("MCP endpoint must use HTTPS without credentials or fragments");
    }
    let (scheme, rest) = value.split_once("://").context("missing MCP authority")?;
    if !scheme.eq_ignore_ascii_case("https") {
        bail!("MCP endpoint must use HTTPS");
    }
    let boundary = rest.find(['/', '?']).unwrap_or(rest.len());
    let (authority, suffix) = rest.split_at(boundary);
    let path = suffix.split_once('?').map_or(suffix, |(path, _)| path);
    if path.contains(['[', ']']) {
        bail!("invalid MCP endpoint path");
    }
    if authority.is_empty() || !authority.is_ascii() || authority.contains(['@', '%']) {
        bail!("invalid MCP endpoint authority");
    }
    let (host, port) = if authority.starts_with('[') {
        let end = authority.find(']').context("invalid MCP IPv6 authority")? + 1;
        if !matches!(parsed.host(), Some(url::Host::Ipv6(_))) {
            bail!("invalid MCP IPv6 host");
        }
        let (host, remainder) = authority.split_at(end);
        let port = if remainder.is_empty() {
            ""
        } else {
            remainder.strip_prefix(':').context("invalid MCP port")?
        };
        (host, port)
    } else {
        let (host, port) = authority.split_once(':').unwrap_or((authority, ""));
        // Match URI's ASCII server-host syntax without adopting Url's numeric
        // host aliases (e.g. 127.1) or decoding percent-encoded DNS names.
        let labels: Vec<_> = host.strip_suffix('.').unwrap_or(host).split('.').collect();
        if labels.iter().any(|label| {
            label.is_empty()
                || !label
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || c == b'-')
                || !label.as_bytes()[0].is_ascii_alphanumeric()
                || !label.as_bytes()[label.len() - 1].is_ascii_alphanumeric()
        }) {
            bail!("invalid MCP hostname");
        }
        if labels.len() > 1
            && !labels.last().unwrap().as_bytes()[0].is_ascii_alphabetic()
            && (host.ends_with('.')
                || labels.len() != 4
                || labels.iter().any(|label| label.parse::<u8>().is_err()))
        {
            bail!("invalid MCP numeric hostname");
        }
        (host, port)
    };
    let port = if port.is_empty() {
        None
    } else {
        if !port.bytes().all(|c| c.is_ascii_digit()) {
            bail!("invalid MCP port");
        }
        Some(port.parse::<u16>().context("invalid MCP port")?)
    };
    let port = match port {
        None | Some(443) => String::new(),
        Some(port) => format!(":{port}"),
    };
    let suffix = if suffix.is_empty() || suffix.starts_with('?') {
        format!("/{suffix}")
    } else {
        suffix.to_owned()
    };
    Ok(format!(
        "https://{}{port}{suffix}",
        host.to_ascii_lowercase()
    ))
}

impl RunAuthentication {
    /// Validate every endpoint before consulting credentials. On cold resume,
    /// use the stored identity without asking the app to choose a new account.
    pub(crate) fn resolve(
        run_dir: &Path,
        servers: &[ServerConfig],
        provider: Option<&SharedTokenProvider>,
        resume: bool,
    ) -> Result<Self> {
        let mut endpoints = BTreeMap::new();
        for server in servers {
            if server.name.trim().is_empty()
                || endpoints
                    .insert(server.name.clone(), endpoint(&server.url)?)
                    .is_some()
            {
                bail!("MCP server names must be nonempty and unique");
            }
        }
        if resume {
            match fs::read(run_dir.join(AUTH_FILE)) {
                Ok(bytes) => {
                    let saved: Self = serde_json::from_slice(&bytes)
                        .context("invalid saved connection authentication")?;
                    saved.validate(&endpoints)?;
                    return Ok(saved);
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
        let mut pins = BTreeMap::new();
        for (name, url) in &endpoints {
            let auth = provider.map_or(ConnectionAuth::Anonymous, |provider| {
                provider.connection_auth(name.clone(), url.clone())
            });
            // Older runs have no grant identity to resume. Keep their manual
            // behavior, but never silently enroll them in a newly signed-in account.
            if resume && matches!(auth, ConnectionAuth::Oauth { .. }) {
                bail!("legacy run has no saved OAuth grant; start a new run after sign-in");
            }
            pins.insert(
                name.clone(),
                PinnedConnection {
                    url: url.clone(),
                    auth,
                },
            );
        }
        let saved = Self {
            version: 1,
            servers: pins,
        };
        saved.validate(&endpoints)?;
        trollm_core::artifacts::write_atomic(
            run_dir,
            AUTH_FILE,
            &serde_json::to_vec_pretty(&saved)?,
        )?;
        Ok(saved)
    }

    fn validate(&self, endpoints: &BTreeMap<String, String>) -> Result<()> {
        if self.version != 1 || !self.servers.keys().eq(endpoints.keys()) {
            bail!("saved authentication does not match the run's MCP connections");
        }
        for (name, pin) in &self.servers {
            if pin.url != endpoints[name] {
                bail!("saved authentication endpoint does not match connection '{name}'");
            }
            match &pin.auth {
                ConnectionAuth::Unavailable => {
                    bail!("connection '{name}' needs sign-in or a manual token saved for this endpoint before this run can start");
                }
                ConnectionAuth::Oauth { grant_id }
                    if grant_id.is_empty()
                        || grant_id.len() > 128
                        || !grant_id.bytes().all(|byte| {
                            byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_')
                        }) =>
                {
                    bail!("invalid saved OAuth grant identity");
                }
                _ => {}
            }
        }
        Ok(())
    }

    pub(crate) fn bind(
        &self,
        config: ServerConfig,
        provider: Option<&SharedTokenProvider>,
    ) -> Result<ServerConfig> {
        let pin = self
            .servers
            .get(&config.name)
            .context("missing connection identity")?;
        if endpoint(&config.url)? != pin.url {
            bail!("connection endpoint does not match saved authentication");
        }
        match &pin.auth {
            ConnectionAuth::Anonymous => Ok(config),
            ConnectionAuth::Bearer => {
                let provider = provider.context("manual connection token unavailable for this endpoint; re-save it in Connections")?;
                if provider.connection_auth(config.name.clone(), pin.url.clone())
                    != ConnectionAuth::Bearer
                {
                    bail!("manual connection changed; start a new run with the selected authentication");
                }
                let token = provider
                    .bearer_token(config.name.clone(), pin.url.clone())
                    .filter(|token| !token.trim().is_empty())
                    .context("manual connection token unavailable for this endpoint; re-save it in Connections")?;
                Ok(config.with_token_getter(Arc::new(move || Some(token.clone()))))
            }
            ConnectionAuth::Oauth { grant_id } => {
                let provider = provider
                    .context("OAuth connection unavailable; reconnect")?
                    .clone();
                let name = config.name.clone();
                let url = pin.url.clone();
                let grant = grant_id.clone();
                Ok(config.with_grant_token_getter(Arc::new(move || {
                    provider.oauth_token(name.clone(), url.clone(), grant.clone())
                })))
            }
            ConnectionAuth::Unavailable => bail!("connection requires sign-in"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TokenProvider;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    struct TestDir(PathBuf);
    impl TestDir {
        fn new() -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let path = std::env::temp_dir().join(format!(
                "trollm-auth-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    struct State {
        selected: Mutex<ConnectionAuth>,
        token: Mutex<String>,
        selections: AtomicUsize,
    }
    struct Provider(Arc<State>);
    impl TokenProvider for Provider {
        fn bearer_token(&self, name: String, url: String) -> Option<String> {
            (name == "browser"
                && url == "https://example.test/mcp"
                && *self.0.selected.lock().unwrap() == ConnectionAuth::Bearer)
                .then(|| self.0.token.lock().unwrap().clone())
        }
        fn connection_auth(&self, _name: String, _url: String) -> ConnectionAuth {
            self.0.selections.fetch_add(1, Ordering::Relaxed);
            self.0.selected.lock().unwrap().clone()
        }
        fn oauth_token(&self, name: String, url: String, grant_id: String) -> Option<String> {
            (name == "browser"
                && url == "https://example.test/mcp"
                && *self.0.selected.lock().unwrap() == ConnectionAuth::Oauth { grant_id })
            .then(|| self.0.token.lock().unwrap().clone())
        }
    }

    fn provider(selected: ConnectionAuth) -> (Arc<State>, SharedTokenProvider) {
        let state = Arc::new(State {
            selected: Mutex::new(selected),
            token: Mutex::new("fixture-private-access-token".into()),
            selections: AtomicUsize::new(0),
        });
        let provider: SharedTokenProvider = Arc::new(Box::new(Provider(state.clone())));
        (state, provider)
    }
    fn server() -> ServerConfig {
        ServerConfig::new("browser", "https://example.test/mcp")
    }
    fn oauth(id: &str) -> ConnectionAuth {
        ConnectionAuth::Oauth {
            grant_id: id.into(),
        }
    }

    #[test]
    fn resume_keeps_grant_while_tokens_refresh_without_recording_secrets() {
        let dir = TestDir::new();
        let (state, provider) = provider(oauth("grant-a"));
        let auth = RunAuthentication::resolve(&dir.0, &[server()], Some(&provider), false).unwrap();
        let config = auth.bind(server(), Some(&provider)).unwrap();
        let get = config.token_getter.unwrap();
        assert_eq!(get().as_deref(), Some("fixture-private-access-token"));
        *state.token.lock().unwrap() = "fixture-refreshed-access-token".into();
        assert_eq!(get().as_deref(), Some("fixture-refreshed-access-token"));
        let resumed =
            RunAuthentication::resolve(&dir.0, &[server()], Some(&provider), true).unwrap();
        assert_eq!(resumed.servers["browser"].auth, oauth("grant-a"));
        assert_eq!(state.selections.load(Ordering::Relaxed), 1);
        let persisted = fs::read_to_string(dir.0.join(AUTH_FILE)).unwrap();
        assert!(persisted.contains("grant-a"));
        assert!(!persisted.contains("access-token"));
        assert!(!persisted.contains("refreshToken"));
    }

    #[test]
    fn replacement_or_logout_never_rebinds_a_running_or_resumed_grant() {
        let dir = TestDir::new();
        let (state, provider) = provider(oauth("grant-a"));
        let auth = RunAuthentication::resolve(&dir.0, &[server()], Some(&provider), false).unwrap();
        let get = auth
            .bind(server(), Some(&provider))
            .unwrap()
            .token_getter
            .unwrap();
        for replacement in [
            ConnectionAuth::Unavailable,
            oauth("grant-b"),
            ConnectionAuth::Bearer,
        ] {
            *state.selected.lock().unwrap() = replacement;
            assert!(get().is_none());
            let resumed =
                RunAuthentication::resolve(&dir.0, &[server()], Some(&provider), true).unwrap();
            assert_eq!(resumed.servers["browser"].auth, oauth("grant-a"));
            assert!(resumed
                .bind(server(), Some(&provider))
                .unwrap()
                .token_getter
                .unwrap()()
            .is_none());
        }
        assert_eq!(state.selections.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn endpoint_and_saved_pin_changes_fail_before_account_selection() {
        let dir = TestDir::new();
        let (state, provider) = provider(oauth("grant-a"));
        let invalid = ServerConfig::new("other", "http://example.test/mcp");
        assert!(
            RunAuthentication::resolve(&dir.0, &[server(), invalid], Some(&provider), false)
                .is_err()
        );
        assert_eq!(state.selections.load(Ordering::Relaxed), 0);
        RunAuthentication::resolve(&dir.0, &[server()], Some(&provider), false).unwrap();
        let changed = ServerConfig::new("browser", "https://elsewhere.test/mcp");
        assert!(RunAuthentication::resolve(&dir.0, &[changed], Some(&provider), true).is_err());
        let path = dir.0.join(AUTH_FILE);
        let mut value: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        value["servers"]["browser"]["auth"]["grant_id"] = "".into();
        fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
        assert!(RunAuthentication::resolve(&dir.0, &[server()], Some(&provider), true).is_err());
        assert_eq!(state.selections.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn pending_signin_and_legacy_oauth_resume_require_explicit_new_run() {
        let dir = TestDir::new();
        let (state, provider) = provider(ConnectionAuth::Unavailable);
        assert!(RunAuthentication::resolve(&dir.0, &[server()], Some(&provider), false).is_err());
        assert!(!dir.0.join(AUTH_FILE).exists());
        *state.selected.lock().unwrap() = oauth("new-login");
        let error =
            RunAuthentication::resolve(&dir.0, &[server()], Some(&provider), true).unwrap_err();
        assert!(error.to_string().contains("no saved OAuth grant"));
        assert!(!dir.0.join(AUTH_FILE).exists());
        *state.selected.lock().unwrap() = ConnectionAuth::Bearer;
        let legacy =
            RunAuthentication::resolve(&dir.0, &[server()], Some(&provider), true).unwrap();
        assert!(legacy
            .bind(server(), Some(&provider))
            .unwrap()
            .token_getter
            .unwrap()()
        .is_some());
    }

    #[test]
    fn anonymous_resume_stays_anonymous_and_manual_resume_cannot_switch_to_oauth() {
        let anonymous = TestDir::new();
        let (state, provider) = provider(ConnectionAuth::Anonymous);
        RunAuthentication::resolve(&anonymous.0, &[server()], Some(&provider), false).unwrap();
        *state.selected.lock().unwrap() = oauth("new-login");
        let resumed =
            RunAuthentication::resolve(&anonymous.0, &[server()], Some(&provider), true).unwrap();
        assert!(resumed
            .bind(server(), Some(&provider))
            .unwrap()
            .token_getter
            .is_none());

        let manual = TestDir::new();
        *state.selected.lock().unwrap() = ConnectionAuth::Bearer;
        RunAuthentication::resolve(&manual.0, &[server()], Some(&provider), false).unwrap();
        *state.selected.lock().unwrap() = oauth("new-login");
        let resumed =
            RunAuthentication::resolve(&manual.0, &[server()], Some(&provider), true).unwrap();
        assert!(resumed.bind(server(), Some(&provider)).is_err());
    }

    #[test]
    fn manual_token_delivery_rechecks_endpoint_after_concurrent_selection_change() {
        // Simulate an edit immediately after selection but before secret delivery.
        // No token getter may be installed for the old endpoint with the new secret.
        struct EditingProvider {
            endpoint: Mutex<String>,
            selections: AtomicUsize,
            deliveries: AtomicUsize,
        }
        impl TokenProvider for EditingProvider {
            fn bearer_token(&self, name: String, url: String) -> Option<String> {
                let endpoint = self.endpoint.lock().unwrap();
                if name == "browser" && url == *endpoint {
                    self.deliveries.fetch_add(1, Ordering::Relaxed);
                    Some("dummy-for-replacement-endpoint".into())
                } else {
                    None
                }
            }
            fn connection_auth(&self, _name: String, _url: String) -> ConnectionAuth {
                if self.selections.fetch_add(1, Ordering::Relaxed) == 1 {
                    *self.endpoint.lock().unwrap() = "https://elsewhere.test/mcp".into();
                }
                ConnectionAuth::Bearer
            }
        }
        let dir = TestDir::new();
        let provider = Arc::new(EditingProvider {
            endpoint: Mutex::new("https://example.test/mcp".into()),
            selections: AtomicUsize::new(0),
            deliveries: AtomicUsize::new(0),
        });
        struct SharedProvider(Arc<EditingProvider>);
        impl TokenProvider for SharedProvider {
            fn bearer_token(&self, name: String, url: String) -> Option<String> {
                self.0.bearer_token(name, url)
            }
            fn connection_auth(&self, name: String, url: String) -> ConnectionAuth {
                self.0.connection_auth(name, url)
            }
        }
        let shared: SharedTokenProvider = Arc::new(Box::new(SharedProvider(provider.clone())));
        let auth = RunAuthentication::resolve(&dir.0, &[server()], Some(&shared), false).unwrap();
        assert!(auth.bind(server(), Some(&shared)).is_err());
        assert_eq!(provider.deliveries.load(Ordering::Relaxed), 0);
        let resumed = RunAuthentication::resolve(&dir.0, &[server()], Some(&shared), true).unwrap();
        assert!(resumed.bind(server(), Some(&shared)).is_err());
        assert_eq!(provider.deliveries.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn manual_binding_rejects_changed_config_and_endpoint_override_before_delivery() {
        let dir = TestDir::new();
        let (_, provider) = provider(ConnectionAuth::Bearer);
        let auth = RunAuthentication::resolve(&dir.0, &[server()], Some(&provider), false).unwrap();
        let changed = ServerConfig::new("browser", "https://elsewhere.test/mcp");
        assert!(auth
            .bind(
                ServerConfig::new("browser", "https://elsewhere.test/mcp"),
                Some(&provider),
            )
            .is_err());
        assert!(RunAuthentication::resolve(&dir.0, &[changed], Some(&provider), true).is_err());
        let fresh_dir = TestDir::new();
        let overridden = ServerConfig::new("browser", "https://elsewhere.test/mcp");
        // Even a stale/misleading selection result cannot deliver an endpoint-bound secret.
        let fresh = RunAuthentication::resolve(
            &fresh_dir.0,
            std::slice::from_ref(&overridden),
            Some(&provider),
            false,
        )
        .unwrap();
        assert!(fresh.bind(overridden, Some(&provider)).is_err());
        let resumed =
            RunAuthentication::resolve(&dir.0, &[server()], Some(&provider), true).unwrap();
        assert_eq!(
            resumed
                .bind(server(), Some(&provider))
                .unwrap()
                .token_getter
                .unwrap()(),
            Some("fixture-private-access-token".into())
        );
    }

    #[derive(serde::Deserialize)]
    struct EndpointCase {
        id: String,
        input: String,
        expected: Option<String>,
    }

    fn endpoint_cases() -> Vec<EndpointCase> {
        serde_json::from_str(include_str!("../tests/fixtures/endpoint-identity.json")).unwrap()
    }

    #[test]
    fn endpoint_identity_matches_shared_android_fixtures() {
        for case in endpoint_cases() {
            assert_eq!(endpoint(&case.input).ok(), case.expected, "{}", case.id);
        }
    }

    #[test]
    fn invalid_endpoint_fixtures_fail_before_any_credential_callback() {
        let dir = TestDir::new();
        let (state, provider) = provider(oauth("grant-a"));
        for case in endpoint_cases()
            .into_iter()
            .filter(|case| case.expected.is_none())
        {
            let invalid = ServerConfig::new("invalid", &case.input);
            assert!(
                RunAuthentication::resolve(&dir.0, &[server(), invalid], Some(&provider), false)
                    .is_err(),
                "{}",
                case.id
            );
            assert_eq!(state.selections.load(Ordering::Relaxed), 0, "{}", case.id);
            assert!(!dir.0.join(AUTH_FILE).exists());
        }
    }

    struct ExactEndpoint {
        url: String,
        mode: ConnectionAuth,
        selections: Mutex<Vec<String>>,
        token_calls: AtomicUsize,
    }
    struct ExactProvider(Arc<ExactEndpoint>);
    impl TokenProvider for ExactProvider {
        fn connection_auth(&self, name: String, url: String) -> ConnectionAuth {
            self.0.selections.lock().unwrap().push(url.clone());
            if name == "browser" && url == self.0.url {
                self.0.mode.clone()
            } else {
                ConnectionAuth::Unavailable
            }
        }
        fn bearer_token(&self, name: String, url: String) -> Option<String> {
            self.0.token_calls.fetch_add(1, Ordering::Relaxed);
            (name == "browser" && url == self.0.url && self.0.mode == ConnectionAuth::Bearer)
                .then(|| "fixture-exact-token".into())
        }
        fn oauth_token(&self, name: String, url: String, grant_id: String) -> Option<String> {
            self.0.token_calls.fetch_add(1, Ordering::Relaxed);
            (name == "browser"
                && url == self.0.url
                && self.0.mode == ConnectionAuth::Oauth { grant_id })
            .then(|| "fixture-exact-token".into())
        }
    }
    fn exact_provider(
        url: &str,
        mode: ConnectionAuth,
    ) -> (Arc<ExactEndpoint>, SharedTokenProvider) {
        let state = Arc::new(ExactEndpoint {
            url: url.into(),
            mode,
            selections: Mutex::new(Vec::new()),
            token_calls: AtomicUsize::new(0),
        });
        let shared: SharedTokenProvider = Arc::new(Box::new(ExactProvider(state.clone())));
        (state, shared)
    }

    #[test]
    fn raw_endpoint_identity_delivers_manual_and_oauth_on_fresh_and_cold_runs() {
        for mode in [ConnectionAuth::Bearer, oauth("exact-grant")] {
            for (input, identity) in [
                (
                    "HTTPS://EXAMPLE.TEST:00443/a/../mcp?tenant=%2F&x=雪",
                    "https://example.test/a/../mcp?tenant=%2F&x=雪",
                ),
                (
                    "https://example.test/a/%2e%2e/mcp?",
                    "https://example.test/a/%2e%2e/mcp?",
                ),
                (
                    "https://[2001:0DB8:0:0:0:0:0:1]:443/café?q=%2f",
                    "https://[2001:0db8:0:0:0:0:0:1]/café?q=%2f",
                ),
            ] {
                let dir = TestDir::new();
                let config = || ServerConfig::new("browser", input);
                let (state, provider) = exact_provider(identity, mode.clone());
                for resume in [false, true] {
                    let auth =
                        RunAuthentication::resolve(&dir.0, &[config()], Some(&provider), resume)
                            .unwrap();
                    assert_eq!(auth.servers["browser"].url, identity);
                    let token = auth
                        .bind(config(), Some(&provider))
                        .unwrap()
                        .token_getter
                        .unwrap()();
                    assert_eq!(token.as_deref(), Some("fixture-exact-token"));
                }
                assert_eq!(state.token_calls.load(Ordering::Relaxed), 2);
                assert!(state
                    .selections
                    .lock()
                    .unwrap()
                    .iter()
                    .all(|url| url == identity));
                assert!(!fs::read_to_string(dir.0.join(AUTH_FILE))
                    .unwrap()
                    .contains("fixture-exact-token"));
            }
        }
    }

    #[test]
    fn collapsed_and_escaped_aliases_never_rebind_or_request_tokens() {
        let original = "https://example.test/a/../mcp?tenant=%2F&x=雪";
        for mode in [ConnectionAuth::Bearer, oauth("exact-grant")] {
            let dir = TestDir::new();
            let (state, provider) = exact_provider(original, mode);
            let auth = RunAuthentication::resolve(
                &dir.0,
                &[ServerConfig::new("browser", original)],
                Some(&provider),
                false,
            )
            .unwrap();
            let saved = fs::read(dir.0.join(AUTH_FILE)).unwrap();
            let selections = state.selections.lock().unwrap().len();
            for alias in [
                "https://example.test/mcp?tenant=%2F&x=雪",
                "https://example.test/a/%2e%2e/mcp?tenant=%2F&x=雪",
                "https://example.test/a/../mcp?tenant=%2f&x=雪",
                "https://example.test/a/../mcp?x=雪&tenant=%2F",
            ] {
                assert!(auth
                    .bind(ServerConfig::new("browser", alias), Some(&provider))
                    .is_err());
                assert!(RunAuthentication::resolve(
                    &dir.0,
                    &[ServerConfig::new("browser", alias)],
                    Some(&provider),
                    true
                )
                .is_err());
                assert_eq!(state.token_calls.load(Ordering::Relaxed), 0);
                assert_eq!(state.selections.lock().unwrap().len(), selections);
                assert_eq!(fs::read(dir.0.join(AUTH_FILE)).unwrap(), saved);
            }
        }
    }

    #[test]
    fn old_collapsed_saved_dot_pin_fails_cold_resume_without_reselection_or_migration() {
        for mode in [ConnectionAuth::Bearer, oauth("exact-grant")] {
            let dir = TestDir::new();
            let (state, provider) = exact_provider("https://example.test/mcp", mode);
            RunAuthentication::resolve(&dir.0, &[server()], Some(&provider), false).unwrap();
            let saved = fs::read(dir.0.join(AUTH_FILE)).unwrap();
            let selections = state.selections.lock().unwrap().len();
            for raw in [
                "https://example.test/a/../mcp",
                "https://example.test/a/%2e%2e/mcp",
            ] {
                assert!(RunAuthentication::resolve(
                    &dir.0,
                    &[ServerConfig::new("browser", raw)],
                    Some(&provider),
                    true
                )
                .is_err());
                assert_eq!(state.selections.lock().unwrap().len(), selections);
                assert_eq!(state.token_calls.load(Ordering::Relaxed), 0);
                assert_eq!(fs::read(dir.0.join(AUTH_FILE)).unwrap(), saved);
            }
            // Ordinary endpoint pins retain the existing cold-resume behavior.
            let resumed =
                RunAuthentication::resolve(&dir.0, &[server()], Some(&provider), true).unwrap();
            assert_eq!(
                resumed
                    .bind(server(), Some(&provider))
                    .unwrap()
                    .token_getter
                    .unwrap()()
                .as_deref(),
                Some("fixture-exact-token")
            );
        }
    }
}
