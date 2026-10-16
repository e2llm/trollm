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
pub struct RootCertificates(Vec<reqwest::Certificate>);

impl RootCertificates {
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
        Ok(Self(certificates))
    }
}

/// One synchronous MCP client session against one server.
pub struct McpClient {
    url: String,
    http: reqwest::blocking::Client,
    token_getter: Option<TokenGetter>,
    require_token: bool,
    session_id: Option<String>,
    next_id: u64,
    initialized: bool,
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
        for certificate in roots.0 {
            builder = builder.add_root_certificate(certificate);
        }
        let http = builder.build().context("building HTTP client")?;
        Ok(Self {
            url,
            http,
            token_getter,
            require_token: false,
            session_id: None,
            next_id: 1,
            initialized: false,
        })
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
                "protocolVersion": "2025-03-26",
                "capabilities": {},
                "clientInfo": { "name": "trollm-mcp", "version": env!("CARGO_PKG_VERSION") }
            }),
            timeout,
        )?;
        let _ = result;
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
        let started = Instant::now();
        self.ensure_session(timeout)?;
        let result = self.rpc_raw(
            "tools/call",
            json!({ "name": tool, "arguments": args }),
            remaining_timeout(timeout, started)?,
        )?;
        if result.get("isError").and_then(Value::as_bool) == Some(true) {
            bail!("MCP tool {tool} reported isError: {result}");
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
        req = self.with_auth(req)?;
        if let Some(timeout) =
            remaining_timeout(Some(timeout.unwrap_or(Duration::from_secs(30))), started)?
        {
            req = req.timeout(timeout);
        }
        let resp = req
            .send()
            .map_err(reqwest::Error::without_url)
            .context("MCP notification POST failed")?;
        let status = resp.status();
        if !status.is_success() && status.as_u16() != 202 {
            bail!("MCP notification {method} failed: HTTP {status}");
        }
        Ok(())
    }

    fn rpc_raw(&mut self, method: &str, params: Value, timeout: Option<Duration>) -> Result<Value> {
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
        req = self.with_auth(req)?;
        if let Some(timeout) =
            remaining_timeout(Some(timeout.unwrap_or(Duration::from_secs(30))), started)?
        {
            req = req.timeout(timeout);
        }
        let resp = req
            .send()
            .map_err(reqwest::Error::without_url)
            .with_context(|| format!("MCP {method} POST failed"))?;
        let status = resp.status();
        if !status.is_success() {
            bail!("MCP {method} failed: HTTP {status}");
        }
        if method == "initialize" {
            if let Some(session) = resp
                .headers()
                .get(MCP_SESSION_HEADER)
                .and_then(|v| v.to_str().ok())
            {
                self.session_id = Some(session.to_string());
            }
        }
        let content_type = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        const MAX_RESPONSE_BYTES: u64 = 32 * 1024 * 1024;
        let mut bytes = Vec::new();
        resp.take(MAX_RESPONSE_BYTES + 1)
            .read_to_end(&mut bytes)
            .context("reading MCP response body")?;
        if bytes.len() as u64 > MAX_RESPONSE_BYTES {
            bail!("MCP response exceeds byte limit");
        }
        let body = String::from_utf8(bytes).context("MCP response is not UTF-8")?;
        let message = if content_type.contains("text/event-stream") {
            parse_sse_response(&body, id)?
        } else {
            serde_json::from_str(&body).context("parsing MCP JSON response")?
        };
        if let Some(error) = message.get("error") {
            if let Some(code) = error.get("code").and_then(Value::as_i64) {
                bail!("MCP {method} returned error code {code}");
            }
            bail!("MCP {method} returned error with invalid or missing code");
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
            let chain = format!("{error:#}");
            assert!(chain.contains(context));
            assert!(chain.contains("timed out"));
            assert!(!chain.contains(&url));
            assert!(!chain.contains("private-endpoint"));
            assert!(!chain.contains("secret-query-token"));
        }
    }
}
