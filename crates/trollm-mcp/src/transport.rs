//! Hand-rolled MCP "streamable HTTP" transport (§9 leaves rmcp vs hand-rolled
//! to us): JSON-RPC 2.0 over POST with initialize/session handling, tools/list,
//! tools/call. Synchronous, on reqwest::blocking, so the public ToolGateway
//! trait stays sync.

use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};
use std::io::Read;
use std::sync::Arc;
use std::time::{Duration, Instant};

pub const MCP_SESSION_HEADER: &str = "mcp-session-id";

pub type TokenGetter = Arc<dyn Fn() -> Option<String> + Send + Sync>;

/// Operator-provisioned trust anchors, added alongside the built-in Mozilla roots.
/// Empty/default leaves the existing trust policy unchanged.
#[derive(Clone, Default)]
pub struct RootCertificates(
    Vec<reqwest::Certificate>,
    Vec<rustls::pki_types::CertificateDer<'static>>,
);

impl RootCertificates {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    pub(crate) fn selected_tls_config(&self) -> Result<rustls::ClientConfig> {
        let mut store =
            rustls::RootCertStore::from_iter(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        for certificate in &self.1 {
            store.add(certificate.clone())?;
        }
        let mut config = rustls::ClientConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()?
        .with_root_certificates(store)
        .with_no_client_auth();
        config.alpn_protocols = vec![b"http/1.1".to_vec()];
        config.enable_early_data = false;
        Ok(config)
    }

    pub(crate) fn blocking_client(&self, timeout: Duration) -> Result<reqwest::blocking::Client> {
        let mut builder = reqwest::blocking::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(timeout);
        for certificate in &self.0 {
            builder = builder.add_root_certificate(certificate.clone());
        }
        builder.build().context("building trusted HTTP client")
    }
    pub fn from_pem(pem: &[u8]) -> Result<Self> {
        let certificates = reqwest::Certificate::from_pem_bundle(pem)
            .context("invalid MCP root certificate PEM")?;
        if certificates.is_empty() {
            bail!("MCP root certificate PEM contains no certificates");
        }
        // PEM decoding alone does not validate DER in reqwest's rustls backend.
        // Building an unsent async client validates the anchors without sockets
        // or a blocking runtime. The runtime clients retain the built-in roots.
        let mut validator = reqwest::Client::builder();
        for certificate in &certificates {
            validator = validator.add_root_certificate(certificate.clone());
        }
        validator.build().context("invalid MCP root certificates")?;
        use rustls::pki_types::pem::PemObject;
        let der = rustls::pki_types::CertificateDer::pem_slice_iter(pem)
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(Self(certificates, der))
    }
}

/// One synchronous MCP client session against one server.
pub struct McpClient {
    url: String,
    http: reqwest::blocking::Client,
    roots: RootCertificates,
    token_getter: Option<TokenGetter>,
    require_token: bool,
    session_id: Option<String>,
    next_id: u64,
    initialized: bool,
    protocol_version: String,
    strict_protocol: bool,
}

impl McpClient {
    pub fn new(url: impl Into<String>, token_getter: Option<TokenGetter>) -> Result<Self> {
        Self::with_root_certificates(url, token_getter, RootCertificates::default())
    }

    pub fn with_root_certificates(
        url: impl Into<String>,
        token_getter: Option<TokenGetter>,
        roots: RootCertificates,
    ) -> Result<Self> {
        Self::build(url.into(), token_getter, false, roots)
    }

    /// Explicit opt-in for loopback HTTP fixtures. Production uses `new`.
    pub fn for_local_test(
        url: impl Into<String>,
        token_getter: Option<TokenGetter>,
    ) -> Result<Self> {
        Self::for_local_test_with_root_certificates(url, token_getter, RootCertificates::default())
    }

    pub(crate) fn for_local_test_with_root_certificates(
        url: impl Into<String>,
        token_getter: Option<TokenGetter>,
        roots: RootCertificates,
    ) -> Result<Self> {
        Self::build(url.into(), token_getter, true, roots)
    }

    pub(crate) fn validate_url(url: &str, allow_loopback_http: bool) -> Result<()> {
        let parsed = reqwest::Url::parse(url).context("invalid MCP endpoint")?;
        let loopback = matches!(parsed.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"));
        if parsed.scheme() != "https"
            && !(allow_loopback_http && parsed.scheme() == "http" && loopback)
        {
            bail!(
                "MCP endpoint must use HTTPS (HTTP permitted only for explicit loopback fixtures)"
            );
        }
        if parsed.host_str().is_none()
            || !parsed.username().is_empty()
            || parsed.password().is_some()
        {
            bail!("MCP endpoint must have a host and no embedded credentials");
        }
        Ok(())
    }

    fn build(
        url: String,
        token_getter: Option<TokenGetter>,
        allow_loopback_http: bool,
        roots: RootCertificates,
    ) -> Result<Self> {
        Self::validate_url(&url, allow_loopback_http)?;
        let mut builder = reqwest::blocking::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(30));
        for certificate in &roots.0 {
            builder = builder.add_root_certificate(certificate.clone());
        }
        let http = builder.build().context("building HTTP client")?;
        Ok(Self {
            url,
            http,
            roots,
            token_getter,
            require_token: false,
            session_id: None,
            next_id: 1,
            initialized: false,
            protocol_version: "2025-03-26".into(),
            strict_protocol: false,
        })
    }

    pub fn with_protocol_version(mut self, version: &str) -> Result<Self> {
        anyhow::ensure!(
            matches!(version, "2025-03-26" | "2025-06-18"),
            "unsupported MCP protocol version"
        );
        self.protocol_version = version.into();
        self.strict_protocol = true;
        Ok(self)
    }

    pub(crate) fn require_grant_credentials(mut self) -> Self {
        self.require_token = true;
        self
    }

    /// Lazily performs the MCP initialize handshake once.
    pub fn ensure_session(&mut self, timeout: Option<Duration>) -> Result<()> {
        if self.initialized {
            return Ok(());
        }
        let started = Instant::now();
        let result = self.rpc_raw(
            "initialize",
            json!({
                "protocolVersion": self.protocol_version,
                "capabilities": {},
                "clientInfo": { "name": "trollm-mcp", "version": env!("CARGO_PKG_VERSION") }
            }),
            timeout,
        )?;
        if self.strict_protocol {
            anyhow::ensure!(
                result.get("protocolVersion").and_then(Value::as_str)
                    == Some(&self.protocol_version),
                "MCP negotiated protocol mismatch"
            );
        }
        self.notify(
            "notifications/initialized",
            json!({}),
            remaining_timeout(timeout, started)?,
        )?;
        self.initialized = true;
        Ok(())
    }

    pub fn list_tools(&mut self, timeout: Option<Duration>) -> Result<Value> {
        self.list_tools_page(None, timeout)
    }

    pub(crate) fn list_tools_page(
        &mut self,
        cursor: Option<&str>,
        timeout: Option<Duration>,
    ) -> Result<Value> {
        let started = Instant::now();
        self.ensure_session(timeout)?;
        self.rpc_raw(
            "tools/list",
            cursor.map_or_else(|| json!({}), |cursor| json!({ "cursor": cursor })),
            remaining_timeout(timeout, started)?,
        )
    }

    pub fn call_tool(
        &mut self,
        tool: &str,
        args: &Value,
        timeout: Option<Duration>,
    ) -> Result<Value> {
        self.call_tool_checked(tool, args, timeout, None)
    }

    /// Generic pre-method check. Prepared native effects use the separate
    /// connection-ready first-write path below.
    pub fn call_tool_checked(
        &mut self,
        tool: &str,
        args: &Value,
        timeout: Option<Duration>,
        before_send: Option<&dyn Fn() -> Result<()>>,
    ) -> Result<Value> {
        self.call_tool_using(tool, args, timeout, before_send, false, None)
    }

    #[cfg(test)]
    pub(crate) fn call_prepared_tool_checked(
        &mut self,
        tool: &str,
        args: &Value,
        timeout: Option<Duration>,
        before_write: &dyn Fn() -> Result<()>,
    ) -> Result<Value> {
        self.call_prepared_tool_acquired(tool, args, timeout, &|| Ok(()), before_write)
    }

    pub(crate) fn call_prepared_tool_acquired(
        &mut self,
        tool: &str,
        args: &Value,
        timeout: Option<Duration>,
        acquire: &dyn Fn() -> Result<()>,
        before_write: &dyn Fn() -> Result<()>,
    ) -> Result<Value> {
        self.call_tool_using(tool, args, timeout, Some(before_write), true, Some(acquire))
    }

    fn call_tool_using(
        &mut self,
        tool: &str,
        args: &Value,
        timeout: Option<Duration>,
        before_send: Option<&dyn Fn() -> Result<()>>,
        first_write: bool,
        acquire: Option<&dyn Fn() -> Result<()>>,
    ) -> Result<Value> {
        let started = Instant::now();
        self.ensure_session(timeout)?;
        let mut result = self.rpc_raw_using(
            "tools/call",
            json!({ "name": tool, "arguments": args }),
            remaining_timeout(timeout, started)?,
            before_send,
            first_write,
            acquire,
        )?;
        if result.get("isError").and_then(Value::as_bool) == Some(true) {
            let tool_context: String = tool
                .chars()
                .take(128)
                .flat_map(char::escape_default)
                .collect();
            // Measure the canonical payload once, even when MCP echoes the
            // structured result as text. Error status is carried by the type.
            if let Some(object) = result.as_object_mut() {
                object.remove("isError");
            }
            let payload = extract_tool_payload(&result);
            return Err(received_response_error(
                &payload,
                "tools/call",
                format!("MCP tool {tool_context} reported isError (remote diagnostic omitted)"),
            ));
        }
        Ok(result)
    }

    fn notify(&mut self, method: &str, params: Value, timeout: Option<Duration>) -> Result<()> {
        let started = Instant::now();
        let body = json!({ "jsonrpc": "2.0", "method": method, "params": params });
        let mut req = self
            .http
            .post(&self.url)
            .header("Accept", "application/json, text/event-stream")
            .header("Content-Type", "application/json")
            .json(&body);
        if let Some(session) = &self.session_id {
            req = req.header(MCP_SESSION_HEADER, session);
        }
        if self.strict_protocol {
            req = req.header("MCP-Protocol-Version", &self.protocol_version);
        }
        req = self.with_auth(req)?;
        if let Some(timeout) =
            remaining_timeout(Some(timeout.unwrap_or(Duration::from_secs(30))), started)?
        {
            req = req.timeout(timeout);
        }
        let resp = req
            .send()
            .map_err(|error| post_send_error(error, "MCP notification POST failed"))?;
        let status = resp.status();
        if !status.is_success() && status.as_u16() != 202 {
            bail!("MCP notification {method} failed: HTTP {status}");
        }
        Ok(())
    }

    fn rpc_raw(&mut self, method: &str, params: Value, timeout: Option<Duration>) -> Result<Value> {
        self.rpc_raw_checked(method, params, timeout, None)
    }

    fn rpc_raw_checked(
        &mut self,
        method: &str,
        params: Value,
        timeout: Option<Duration>,
        before_send: Option<&dyn Fn() -> Result<()>>,
    ) -> Result<Value> {
        self.rpc_raw_using(method, params, timeout, before_send, false, None)
    }

    fn rpc_raw_using(
        &mut self,
        method: &str,
        params: Value,
        timeout: Option<Duration>,
        before_send: Option<&dyn Fn() -> Result<()>>,
        first_write: bool,
        acquire: Option<&dyn Fn() -> Result<()>>,
    ) -> Result<Value> {
        let started = Instant::now();
        let id = self.next_id;
        self.next_id += 1;
        let body = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        let mut req = self
            .http
            .post(&self.url)
            .header("Accept", "application/json, text/event-stream")
            .header("Content-Type", "application/json")
            .json(&body);
        if let Some(session) = &self.session_id {
            req = req.header(MCP_SESSION_HEADER, session.clone());
        }
        if self.strict_protocol {
            req = req.header("MCP-Protocol-Version", &self.protocol_version);
        }
        req = self.with_auth(req)?;
        if !first_write {
            if let Some(check) = before_send {
                check()?;
            }
        }
        if let Some(timeout) =
            remaining_timeout(Some(timeout.unwrap_or(Duration::from_secs(30))), started)?
        {
            req = req.timeout(timeout);
        }
        let (status, headers, bytes) = if first_write {
            #[cfg(any(target_os = "linux", target_os = "android"))]
            {
                let request = req.build().map_err(|error| {
                    post_send_error(error, "MCP selected request building failed")
                })?;
                let authorization = request
                    .headers()
                    .get(reqwest::header::AUTHORIZATION)
                    .cloned();
                let acquisition = || {
                    // A refresh or credential-store wait may occur here. Require
                    // the exact credential already bound to this request, then
                    // run native authority/clock/cancel checks after that wait.
                    let current =
                        self.with_auth(self.http.post(&self.url))?
                            .build()
                            .map_err(|error| {
                                post_send_error(
                                    error,
                                    "MCP selected credential request building failed",
                                )
                            })?;
                    anyhow::ensure!(
                        current.headers().get(reqwest::header::AUTHORIZATION)
                            == authorization.as_ref(),
                        "selected credential changed before first write"
                    );
                    if let Some(acquire) = acquire {
                        acquire()?;
                    }
                    Ok(())
                };
                let response = crate::checked_write::send_acquired(
                    request,
                    &self.roots,
                    remaining_timeout(Some(timeout.unwrap_or(Duration::from_secs(30))), started)?
                        .context("selected transport deadline unavailable")?,
                    &acquisition,
                    before_send.context("selected final write check unavailable")?,
                )?;
                (response.status, response.headers, response.bytes)
            }
            #[cfg(not(any(target_os = "linux", target_os = "android")))]
            {
                bail!("selected first-write transport unsupported on this platform");
            }
        } else {
            let resp = req
                .send()
                .map_err(|error| post_send_error(error, &format!("MCP {method} POST failed")))?;
            let status = resp.status().as_u16();
            let headers = resp.headers().clone();
            const MAX_RESPONSE_BYTES: u64 = 32 * 1024 * 1024;
            let mut bytes = Vec::new();
            resp.take(MAX_RESPONSE_BYTES + 1)
                .read_to_end(&mut bytes)
                .context("reading MCP response body")?;
            ensure_response_size(&bytes)?;
            (status, headers, bytes)
        };
        if !(200..300).contains(&status) {
            bail!("MCP {method} failed: HTTP {status}");
        }
        if method == "initialize" {
            if let Some(session) = headers
                .get(MCP_SESSION_HEADER)
                .and_then(|v| v.to_str().ok())
            {
                self.session_id = Some(session.to_string());
            }
        }
        let content_type = headers
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        let body = String::from_utf8(bytes).context("MCP response is not UTF-8")?;
        let message = if content_type.contains("text/event-stream") {
            parse_sse_response(&body, id)?
        } else {
            serde_json::from_str(&body).context("parsing MCP JSON response")?
        };
        if self.strict_protocol {
            anyhow::ensure!(
                message.get("jsonrpc").and_then(Value::as_str) == Some("2.0")
                    && message.get("id").and_then(Value::as_u64) == Some(id),
                "MCP response identity mismatch"
            );
        }
        if let Some(error) = message.get("error") {
            if let Some(code) = error.get("code").and_then(Value::as_i64) {
                return Err(received_response_error(
                    error,
                    method,
                    format!("MCP {method} returned error code {code}"),
                ));
            }
            return Err(received_response_error(
                error,
                method,
                format!("MCP {method} returned error with invalid or missing code"),
            ));
        }
        message
            .get("result")
            .cloned()
            .ok_or_else(|| anyhow!("MCP {method} response had no result field"))
    }

    fn with_auth(
        &self,
        req: reqwest::blocking::RequestBuilder,
    ) -> Result<reqwest::blocking::RequestBuilder> {
        if self.require_token {
            return Ok(req.bearer_auth(required_grant_token(self.token_getter.as_ref())?));
        }
        Ok(match &self.token_getter {
            Some(getter) => match getter() {
                Some(token) => req.bearer_auth(token),
                None => req,
            },
            None => req,
        })
    }
}

pub(crate) fn required_grant_token(getter: Option<&TokenGetter>) -> Result<String> {
    getter
        .and_then(|getter| getter())
        .filter(|token| !token.trim().is_empty())
        .context("MCP connection is no longer authorized; reconnect this connection")
}

/// Keep a safe category in the outer display: callers may discard source chains.
fn post_send_error(error: reqwest::Error, context: &str) -> anyhow::Error {
    let category = if error.is_timeout() {
        "timeout"
    } else if error.is_connect() {
        // A connection error alone does not distinguish DNS, TLS or socket failure.
        "connection failure"
    } else if error.is_builder() {
        "request-building failure"
    } else {
        "other transport failure"
    };
    anyhow::Error::new(error.without_url()).context(format!("{context} ({category})"))
}

fn remaining_timeout(timeout: Option<Duration>, started: Instant) -> Result<Option<Duration>> {
    match timeout {
        None => Ok(None),
        Some(timeout) => Ok(Some(
            timeout
                .checked_sub(started.elapsed())
                .filter(|t| !t.is_zero())
                .context("MCP call timeout exhausted")?,
        )),
    }
}

/// Extracts the JSON-RPC response with matching `id` from an SSE stream body.
fn parse_sse_response(body: &str, id: u64) -> Result<Value> {
    let mut data_lines: Vec<&str> = Vec::new();
    let mut events: Vec<String> = Vec::new();
    for line in body.lines() {
        if line.is_empty() {
            if !data_lines.is_empty() {
                events.push(data_lines.join("\n"));
                data_lines.clear();
            }
        } else if let Some(data) = line.strip_prefix("data:") {
            data_lines.push(data.strip_prefix(' ').unwrap_or(data));
        }
    }
    if !data_lines.is_empty() {
        events.push(data_lines.join("\n"));
    }
    for event in events {
        if let Ok(value) = serde_json::from_str::<Value>(&event) {
            if value.get("id").and_then(Value::as_u64) == Some(id) {
                return Ok(value);
            }
        }
    }
    bail!("no JSON-RPC response with id {id} found in SSE stream")
}

/// Unwraps a tools/call result into the payload the model should see: prefers
/// `structuredContent`, else parses the first text content item as JSON,
/// else returns the text as a string.
pub fn extract_tool_payload(result: &Value) -> Value {
    // Keep the status available even to callers using this helper directly.
    if result.get("isError").and_then(Value::as_bool) == Some(true) {
        return result.clone();
    }
    if let Some(structured) = result.get("structuredContent") {
        return structured.clone();
    }
    if let Some(content) = result.get("content").and_then(Value::as_array) {
        let texts: Vec<&str> = content
            .iter()
            .filter(|c| c.get("type").and_then(Value::as_str) == Some("text"))
            .filter_map(|c| c.get("text").and_then(Value::as_str))
            .collect();
        if let Some(first) = texts.first() {
            if let Ok(parsed) = serde_json::from_str::<Value>(first) {
                return parsed;
            }
        }
        if !texts.is_empty() {
            return Value::String(texts.join("\n"));
        }
    }
    result.clone()
}

/// Locates the items array inside a tool payload: the first array-of-objects
/// field of an object payload, or the payload itself if it is an array.
pub fn find_items_field(payload: &Value) -> Option<(String, Vec<Value>)> {
    match payload {
        Value::Array(items) => Some(("items".to_string(), items.clone())),
        Value::Object(map) => {
            for (key, value) in map {
                if let Value::Array(items) = value {
                    if items.iter().all(|i| i.is_object()) {
                        return Some((key.clone(), items.clone()));
                    }
                }
            }
            None
        }
        _ => None,
    }
}

fn ensure_response_size(bytes: &[u8]) -> Result<()> {
    anyhow::ensure!(
        bytes.len() <= 32 * 1024 * 1024,
        "MCP response exceeds byte limit"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    #[test]
    fn grant_callback_time_exhausts_rpc_and_notification_budget_before_network() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}/mcp", listener.local_addr().unwrap());
        let getter: TokenGetter = Arc::new(|| {
            std::thread::sleep(Duration::from_millis(20));
            Some("fixture-access".into())
        });
        let mut client = McpClient::for_local_test(url, Some(getter))
            .unwrap()
            .require_grant_credentials();
        let timeout = Some(Duration::from_millis(5));
        for result in [
            client.rpc_raw("tools/list", json!({}), timeout).map(|_| ()),
            client.notify("notifications/initialized", json!({}), timeout),
        ] {
            assert!(result
                .unwrap_err()
                .to_string()
                .contains("timeout exhausted"));
        }
        assert_eq!(
            listener.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
    }

    #[test]
    fn grant_notifications_and_requests_refuse_missing_or_blank_credentials() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}/mcp", listener.local_addr().unwrap());
        for token in [None, Some(" \t\n".to_string())] {
            let getter: TokenGetter = Arc::new(move || token.clone());
            let mut client = McpClient::for_local_test(&url, Some(getter))
                .unwrap()
                .require_grant_credentials();
            for result in [
                client.rpc_raw("initialize", json!({}), None).map(|_| ()),
                client.notify("notifications/initialized", json!({}), None),
            ] {
                assert!(result
                    .unwrap_err()
                    .to_string()
                    .contains("reconnect this connection"));
            }
        }
        assert_eq!(
            listener.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
    }

    #[test]
    fn send_error_chains_omit_endpoint_and_query_credentials() {
        // Keep the port reserved but do not serve requests, forcing a bounded
        // request timeout rather than racing another process for a closed port.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!(
            "http://{}/private-endpoint?token=secret-query-token",
            listener.local_addr().unwrap()
        );
        let mut client = McpClient::for_local_test(&url, None).unwrap();
        let timeout = Some(Duration::from_millis(50));
        let rpc_error = client
            .rpc_raw("tools/list", json!({}), timeout)
            .unwrap_err();
        let notification_error = client
            .notify("notifications/initialized", json!({}), timeout)
            .unwrap_err();
        for (error, context) in [
            (rpc_error, "MCP tools/list POST failed"),
            (notification_error, "MCP notification POST failed"),
        ] {
            let display = error.to_string();
            assert_eq!(display, format!("{context} (timeout)"));
            let chain = format!("{error:#}");
            assert!(chain.contains(context));
            assert!(chain.contains("timed out"));
            assert!(!chain.contains(&url));
            assert!(!chain.contains("private-endpoint"));
            assert!(!chain.contains("secret-query-token"));
        }
    }

    #[test]
    fn builder_send_errors_keep_safe_category_without_session_or_endpoint() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!(
            "http://{}/private-endpoint?token=secret-query-token",
            listener.local_addr().unwrap()
        );
        let mut client = McpClient::for_local_test(&url, None).unwrap();
        // Invalid header syntax makes send fail before touching the socket.
        client.session_id = Some("secret-session\r\ninvalid".into());
        for (error, context) in [
            (
                client.rpc_raw("tools/call", json!({}), None).unwrap_err(),
                "MCP tools/call POST failed",
            ),
            (
                client
                    .notify("notifications/initialized", json!({}), None)
                    .unwrap_err(),
                "MCP notification POST failed",
            ),
        ] {
            assert_eq!(
                error.to_string(),
                format!("{context} (request-building failure)")
            );
            let chain = format!("{error:#}");
            for secret in [
                &url,
                "private-endpoint",
                "secret-query-token",
                "secret-session",
            ] {
                assert!(!chain.contains(secret));
            }
        }
        assert_eq!(
            listener.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
    }
}

/// Preserve numeric response size while keeping remote diagnostics out of errors.
#[derive(Debug)]
pub(crate) struct ReceivedResponseError {
    pub compact_json_utf8_bytes: u64,
    pub origin: String,
    detail: String,
}
impl std::fmt::Display for ReceivedResponseError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.detail)
    }
}
impl std::error::Error for ReceivedResponseError {}
fn received_response_error(value: &Value, origin: &str, detail: String) -> anyhow::Error {
    match crate::json_size::compact_bytes(value) {
        Ok(bytes) => ReceivedResponseError {
            compact_json_utf8_bytes: bytes,
            origin: origin.to_string(),
            detail,
        }
        .into(),
        Err(error) => error.context(detail),
    }
}
