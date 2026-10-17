//! Minimal hand-rolled HTTP/1.1 JSON-RPC mock MCP server on
//! std::net::TcpListener (no external test dependencies).

use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

type RequestAuth = Mutex<Vec<(String, Option<String>)>>;

pub struct MockMcpServer {
    pub url: String,
    pub counts: Arc<Mutex<HashMap<String, usize>>>,
    // Shared fixture: catalog assertions are used only by MCP integration binaries.
    #[allow(dead_code)]
    pub list_params: Arc<Mutex<Vec<Value>>>,
    #[allow(dead_code)] // Credential assertions are used only in MCP gateway tests.
    pub request_auth: Arc<RequestAuth>,
    #[allow(dead_code)] // Only result-admission tests override upstream payloads.
    pub tool_payloads: Arc<Mutex<HashMap<String, Value>>>,
    _handle: JoinHandle<()>,
}

impl MockMcpServer {
    pub fn start() -> Self {
        Self::start_with_session(true)
    }

    pub fn start_with_session(stateful: bool) -> Self {
        Self::start_configured(stateful, None)
    }

    /// Override tools/list's JSON-RPC result/error fields; the fixture fills the ID.
    #[allow(dead_code)] // Some integration binaries use only the default catalog.
    pub fn start_with_tools_response(response: Value) -> Self {
        Self::start_with_tools_responses(vec![response])
    }

    /// Script tools/list pages, repeating the last envelope if exhausted.
    #[allow(dead_code)] // Some integration binaries use only the default catalog.
    pub fn start_with_tools_responses(responses: Vec<Value>) -> Self {
        assert!(
            !responses.is_empty(),
            "tools/list responses must not be empty"
        );
        Self::start_configured(true, Some(responses))
    }

    fn start_configured(stateful: bool, tools_responses: Option<Vec<Value>>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock server");
        let addr = listener.local_addr().expect("local addr");
        let counts = Arc::new(Mutex::new(HashMap::new()));
        let counts_thread = counts.clone();
        let list_params = Arc::new(Mutex::new(Vec::new()));
        let list_params_thread = list_params.clone();
        let request_auth = Arc::new(Mutex::new(Vec::new()));
        let request_auth_thread = request_auth.clone();
        let tool_payloads = Arc::new(Mutex::new(HashMap::new()));
        let tool_payloads_thread = tool_payloads.clone();
        let handle = std::thread::spawn(move || {
            for stream in listener.incoming() {
                match stream {
                    Ok(stream) => handle_connection(
                        stream,
                        &counts_thread,
                        stateful,
                        tools_responses.as_deref(),
                        &list_params_thread,
                        &request_auth_thread,
                        &tool_payloads_thread,
                    ),
                    Err(_) => break,
                }
            }
        });
        Self {
            url: format!("http://{addr}/mcp"),
            counts,
            list_params,
            request_auth,
            tool_payloads,
            _handle: handle,
        }
    }

    /// Number of requests seen for a key like "call:list_issues" or "list".
    pub fn count(&self, key: &str) -> usize {
        *self.counts.lock().unwrap().get(key).unwrap_or(&0)
    }
}

fn bump(counts: &Arc<Mutex<HashMap<String, usize>>>, key: &str) {
    *counts.lock().unwrap().entry(key.to_string()).or_insert(0) += 1;
}

fn handle_connection(
    stream: TcpStream,
    counts: &Arc<Mutex<HashMap<String, usize>>>,
    stateful: bool,
    tools_responses: Option<&[Value]>,
    list_params: &Arc<Mutex<Vec<Value>>>,
    request_auth: &RequestAuth,
    tool_payloads: &Mutex<HashMap<String, Value>>,
) {
    let mut reader = BufReader::new(stream.try_clone().expect("clone stream"));
    let mut writer = stream;

    // Request line + headers.
    let mut content_length = 0usize;
    let mut session = None;
    let mut authorization = None;
    loop {
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) | Err(_) => return,
            Ok(_) => {}
        }
        let trimmed = line.trim_end();
        if trimmed.is_empty() {
            break;
        }
        if let Some((name, value)) = trimmed.split_once(':') {
            if name.trim().eq_ignore_ascii_case("authorization") {
                authorization = Some(value.trim().to_string());
            }
            if name.trim().eq_ignore_ascii_case("mcp-session-id") {
                session = Some(value.trim().to_string());
            }
            if name.trim().eq_ignore_ascii_case("content-length") {
                content_length = value.trim().parse().unwrap_or(0);
            }
        }
    }
    let mut body = vec![0u8; content_length];
    if reader.read_exact(&mut body).is_err() {
        return;
    }
    let request: Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(_) => return,
    };
    let method = request.get("method").and_then(Value::as_str).unwrap_or("");
    let id = request.get("id").cloned();
    request_auth
        .lock()
        .unwrap()
        .push((method.to_string(), authorization));

    if stateful && method != "initialize" && session.as_deref() != Some("mock-session") {
        respond(&mut writer, 400, "missing session", None);
        return;
    }
    match method {
        "initialize" => {
            bump(counts, "initialize");
            let result = json!({
                "protocolVersion": "2025-03-26",
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "mock-mcp", "version": "0.1.0" }
            });
            respond(
                &mut writer,
                200,
                &rpc_result(id, result),
                if stateful { Some("mock-session") } else { None },
            );
        }
        m if m.starts_with("notifications/") => {
            respond(&mut writer, 202, "", None);
        }
        "tools/list" => {
            bump(counts, "list");
            let page = {
                let mut params = list_params.lock().unwrap();
                params.push(request.get("params").cloned().unwrap_or(Value::Null));
                params.len() - 1
            };
            if let Some(responses) = tools_responses {
                let mut response = responses[page.min(responses.len() - 1)].clone();
                response["jsonrpc"] = json!("2.0");
                response["id"] = id.unwrap_or(Value::Null);
                respond(&mut writer, 200, &response.to_string(), None);
                return;
            }
            let result = json!({ "tools": [
                { "name": "list_issues", "inputSchema": { "type": "object" } },
                { "name": "get_issue", "inputSchema": { "type": "object" } },
                { "name": "summarize", "inputSchema": { "type": "object" } },
                { "name": "close_issue", "inputSchema": { "type": "object" } },
                { "name": "list_issues_alt", "inputSchema": { "type": "object" } },
                { "name": "redirect", "inputSchema": { "type": "object" } },
                { "name": "drop_mutation", "inputSchema": { "type": "object" } }
            ]});
            respond(&mut writer, 200, &rpc_result(id, result), None);
        }
        "tools/call" => {
            let params = request.get("params").cloned().unwrap_or(Value::Null);
            let name = params.get("name").and_then(Value::as_str).unwrap_or("");
            bump(counts, &format!("call:{name}"));
            let args = params.get("arguments").cloned().unwrap_or(json!({}));
            if args.get("httpStatus").and_then(Value::as_u64) == Some(401) {
                respond(&mut writer, 401, "unauthorized", None);
                return;
            }
            if name == "drop_mutation" {
                return;
            }
            if name == "redirect" {
                let target = args["target"].as_str().unwrap();
                let _ = writer.write_all(format!("HTTP/1.1 302 Found\r\nlocation: {target}\r\ncontent-length: 0\r\nconnection: close\r\n\r\n").as_bytes());
                return;
            }
            if let Some(kind) = args.get("toolError").and_then(Value::as_str) {
                let result = match kind {
                    "near_cap" => {
                        json!({"content": [{"type":"text", "text": format!("credential-secret-{}", "x".repeat(32 * 1024 * 1024 - 2048))}], "isError": true})
                    }
                    "structured" => {
                        json!({"structuredContent": {"failure": true}, "isError": true})
                    }
                    "json_text" => {
                        json!({"content": [{"type":"text", "text":"{\"failure\":true}"}], "isError":true})
                    }
                    _ => {
                        json!({"content": [{"type":"text", "text":"business failure"}], "isError":true})
                    }
                };
                respond(&mut writer, 200, &rpc_result(id, result), None);
                return;
            }
            let payload = tool_payloads
                .lock()
                .unwrap()
                .get(name)
                .cloned()
                .unwrap_or_else(|| tool_payload(name, &args));
            let result = json!({
                "content": [{ "type": "text", "text": payload.to_string() }],
                "structuredContent": payload,
                "isError": false
            });
            respond(&mut writer, 200, &rpc_result(id, result), None);
        }
        _ => {
            let err = json!({ "code": -32601, "message": "method not found" });
            respond(
                &mut writer,
                200,
                &json!({ "jsonrpc": "2.0", "id": id, "error": err }).to_string(),
                None,
            );
        }
    }
}

fn rpc_result(id: Option<Value>, result: Value) -> String {
    json!({ "jsonrpc": "2.0", "id": id.unwrap_or(Value::Null), "result": result }).to_string()
}

/// Scripted tool behavior.
fn tool_payload(name: &str, args: &Value) -> Value {
    match name {
        // 3 pages x 2 items; cursor is a 1-based page number.
        "list_issues" | "list_issues_alt" => {
            let page = args.get("cursor").and_then(Value::as_u64).unwrap_or(1);
            let items: Vec<Value> = (1..=2)
                .map(|i| {
                    let id = (page - 1) * 2 + i;
                    json!({
                        "id": id,
                        "title": format!("Issue {id}"),
                        "body": format!("Full body text of issue {id}, deliberately verbose.")
                    })
                })
                .collect();
            json!({
                "issues": items,
                "cursor": if args["query"] == "cycle" { json!(2) } else if args["query"] == "endless" || page < 3 { json!(page + 1) } else { Value::Null }
            })
        }
        "get_issue" => {
            let id = args.get("id").cloned().unwrap_or(json!(0));
            json!({ "id": id, "title": format!("Issue {id}"), "body": "live body" })
        }
        "summarize" => {
            let query = args.get("query").and_then(Value::as_str).unwrap_or("");
            json!({ "summary": format!("summary of {query}") })
        }
        "close_issue" => {
            let id = args.get("id").cloned().unwrap_or(json!(0));
            json!({ "ok": true, "id": id })
        }
        other => json!({ "error": format!("unknown tool {other}") }),
    }
}

fn respond(writer: &mut TcpStream, status: u16, body: &str, session: Option<&str>) {
    let reason = match status {
        200 => "OK",
        202 => "Accepted",
        _ => "Error",
    };
    let mut headers = format!(
        "HTTP/1.1 {status} {reason}\r\ncontent-length: {}\r\nconnection: close\r\n",
        body.len()
    );
    if status == 200 {
        headers.push_str("content-type: application/json\r\n");
    }
    if let Some(session) = session {
        headers.push_str(&format!("mcp-session-id: {session}\r\n"));
    }
    let _ = writer.write_all(format!("{headers}\r\n{body}").as_bytes());
    let _ = writer.flush();
}

/// Tiny unique temp dir (no external tempfile crate).
pub struct TempDir(pub PathBuf);

static TEMP_COUNTER: AtomicUsize = AtomicUsize::new(0);

impl TempDir {
    pub fn new(tag: &str) -> Self {
        let n = TEMP_COUNTER.fetch_add(1, Ordering::SeqCst);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let path = std::env::temp_dir().join(format!(
            "trollm-mcp-test-{tag}-{}-{nanos}-{n}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).expect("create temp dir");
        Self(path)
    }

    pub fn path(&self) -> &PathBuf {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
