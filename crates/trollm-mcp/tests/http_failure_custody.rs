// Actual loopback HTTP boundary and gateway private custody. No model or actor.
#[allow(dead_code, unused_imports)]
mod common;
use common::TempDir;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use trollm_mcp::{EventSink, McpGateway, PolicySet, ServerConfig, ToolCall, ToolGateway};

fn exercise(fail_at: &str, body: &[u8], declared_extra: usize, blocked_custody: bool) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/mcp", listener.local_addr().unwrap());
    let failed_method = fail_at.to_string();
    let original_body = body.to_vec();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let observed = requests.clone();
    let peer = std::thread::spawn(move || loop {
        let (mut socket, _) = listener.accept().unwrap();
        socket
            .set_read_timeout(Some(std::time::Duration::from_secs(2)))
            .unwrap();
        let mut input = BufReader::new(&mut socket);
        let mut length = 0;
        loop {
            let mut line = String::new();
            input.read_line(&mut line).unwrap();
            if line == "\r\n" {
                break;
            }
            if let Some((name, value)) = line.split_once(':') {
                if name.eq_ignore_ascii_case("content-length") {
                    length = value.trim().parse().unwrap();
                }
            }
        }
        let mut bytes = vec![0; length];
        input.read_exact(&mut bytes).unwrap();
        let request: Value = serde_json::from_slice(&bytes).unwrap();
        let method = request["method"].as_str().unwrap();
        observed.lock().unwrap().push(method.to_owned());
        drop(input);
        if method == failed_method {
            write!(socket, "HTTP/1.1 421 Misdirected Request\r\nServer: envoy\r\nContent-Type: application/octet-stream\r\nSet-Cookie: private-cookie\r\nAuthorization: remote-secret\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", original_body.len()+declared_extra).unwrap();
            socket.write_all(&original_body).unwrap();
            break;
        }
        if method == "notifications/initialized" {
            socket
                .write_all(
                    b"HTTP/1.1 202 Accepted\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .unwrap();
            continue;
        }
        let result = if method == "initialize" {
            json!({"protocolVersion":"2025-03-26","capabilities":{"tools":{}},"serverInfo":{"name":"declared","version":"1"}})
        } else {
            json!({"tools":[{"name":"act","inputSchema":{"type":"object"}}]})
        };
        let reply =
            serde_json::to_vec(&json!({"jsonrpc":"2.0","id":request["id"],"result":result}))
                .unwrap();
        write!(socket, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",reply.len()).unwrap();
        socket.write_all(&reply).unwrap();
    });
    let dirs = TempDir::new("http-failure-custody");
    let artifacts = dirs.path().join("artifacts");
    if blocked_custody {
        std::fs::write(&artifacts, b"blocked-directory").unwrap();
    }
    let events = Arc::new(Mutex::new(Vec::<Value>::new()));
    let target = events.clone();
    let sink: EventSink = Box::new(move |event| {
        target.lock().unwrap().push(serde_json::to_value(event)?);
        Ok(())
    });
    let mut gateway = McpGateway::new(
        vec![ServerConfig::for_local_test("remote", url)],
        PolicySet::from_yaml_str("remote.act: {class: mutation, passthrough: true}").unwrap(),
        dirs.path().join("cache"),
        artifacts.clone(),
        Some(sink),
    )
    .unwrap();
    gateway.set_call_context(Some(trollm_mcp::CallContext {
        epoch: "original-epoch".into(),
        attempt: 2,
        call: 7,
    }));
    let error = gateway
        .call(&ToolCall {
            server: "remote".into(),
            tool: "act".into(),
            args: json!({"value":"original"}),
        })
        .unwrap_err();
    let public = format!("{error:?}");
    assert!(public.contains("MCP_HTTP_RESPONSE_REFUSED") && public.contains("421"));
    for private in [
        "private-cookie",
        "remote-secret",
        "misdirected body",
        "secret payload",
    ] {
        assert!(!public.contains(private));
    }
    peer.join().unwrap();
    assert_eq!(
        requests
            .lock()
            .unwrap()
            .iter()
            .filter(|m| *m == fail_at)
            .count(),
        1
    );
    if blocked_custody {
        assert!(public.contains("MCP_HTTP_DIAGNOSTIC_UNAVAILABLE"));
        assert!(!events
            .lock()
            .unwrap()
            .iter()
            .any(|v| v["type"] == "http_response_refused"));
        return;
    }
    let files: Vec<_> = std::fs::read_dir(&artifacts)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    assert_eq!(files.len(), 1);
    let raw = std::fs::read(&files[0]).unwrap();
    let retained: Value = serde_json::from_slice(&raw).unwrap();
    assert_eq!(retained["server"], "remote");
    assert_eq!(retained["tool"], "act");
    assert_eq!(
        retained["callContext"],
        json!({"epoch":"original-epoch","attempt":2,"call":7})
    );
    let response = &retained["response"];
    assert_eq!(response["rpcMethod"], fail_at);
    assert_eq!(response["status"], 421);
    assert_eq!(response["bodyComplete"], declared_extra == 0);
    assert_eq!(
        response["bodyTruncated"],
        declared_extra != 0 || body.len() > 65536
    );
    let prefix: Vec<u8> = serde_json::from_value(response["bodyPrefixBytes"].clone()).unwrap();
    assert_eq!(prefix, &body[..body.len().min(65536)]);
    assert_eq!(
        response["bodyPrefixSha256"],
        format!("{:x}", Sha256::digest(&prefix))
    );
    if declared_extra == 0 {
        assert_eq!(
            response["bodySha256"],
            format!("{:x}", Sha256::digest(body))
        );
    } else {
        assert!(response["bodySha256"].is_null());
    }
    let text = String::from_utf8(raw.clone()).unwrap();
    assert!(!text.contains("private-cookie") && !text.contains("remote-secret"));
    let event = events
        .lock()
        .unwrap()
        .iter()
        .find(|v| v["type"] == "http_response_refused")
        .unwrap()
        .clone();
    assert_eq!(
        event["diagnostic_sha256"],
        format!("{:x}", Sha256::digest(raw))
    );
    assert_eq!(event["status"], 421);
    assert_eq!(event["call_context"], retained["callContext"]);
    assert_eq!(event["args_hash"], retained["argsHash"]);
    assert!(!event.to_string().contains("bodyPrefixBytes"));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&files[0]).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}

#[test]
fn original_non_json_refusal_after_single_write_retains_binary_and_scoped_event() {
    exercise("tools/call", b"misdirected body\0\xff", 0, false);
}
#[test]
fn original_failure_before_act_retains_initialize_notification_and_catalog_stage() {
    for method in ["initialize", "notifications/initialized", "tools/list"] {
        exercise(method, b"secret payload", 0, false);
    }
}
#[test]
fn original_response_prefix_is_bounded_and_truncation_is_explicit() {
    exercise("tools/call", &vec![255; 70000], 0, false);
}
#[test]
fn short_response_keeps_partial_original_without_fabricating_complete_hash() {
    exercise("tools/call", b"partial", 50, false);
}
#[test]
fn private_custody_failure_preserves_primary_http_refusal_without_remote_detail() {
    exercise("initialize", b"secret payload", 0, true);
}
