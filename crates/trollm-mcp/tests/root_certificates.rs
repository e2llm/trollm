//! Additional operator trust must preserve HTTPS chain and hostname validation.

#[path = "common/tls_fixture.rs"]
mod fixture;

use anyhow::Result;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use rustls::{ServerConnection, StreamOwned};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use trollm_mcp::transport::McpClient;
use trollm_mcp::{McpGateway, PolicySet, RootCertificates, ServerConfig, ToolGateway};

fn https_server(leaf: &[u8], key: &[u8], host: &str) -> (String, JoinHandle<()>) {
    let config = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_no_client_auth()
    .with_single_cert(
        vec![CertificateDer::from(leaf.to_vec())],
        PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.to_vec())),
    )
    .unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let url = format!(
        "https://{host}:{}/mcp",
        listener.local_addr().unwrap().port()
    );
    let handle = std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(3);
        // initialize, initialized notification, tools/list. A failed TLS
        // handshake ends the fixture without waiting for subsequent requests.
        for _ in 0..3 {
            let socket = loop {
                match listener.accept() {
                    Ok((socket, _)) => break socket,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(Instant::now() < deadline, "TLS fixture accept timed out");
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    Err(error) => panic!("TLS fixture accept failed: {error}"),
                }
            };
            socket
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            socket
                .set_write_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let tls = StreamOwned::new(
                ServerConnection::new(Arc::new(config.clone())).unwrap(),
                socket,
            );
            let mut reader = BufReader::new(tls);
            let mut length = 0;
            loop {
                let mut line = String::new();
                match reader.read_line(&mut line) {
                    Ok(0) | Err(_) => return,
                    Ok(_) => {}
                }
                if line == "\r\n" {
                    break;
                }
                if let Some((name, value)) = line.split_once(':') {
                    if name.eq_ignore_ascii_case("content-length") {
                        length = value.trim().parse::<usize>().unwrap();
                    }
                }
            }
            let mut body = vec![0; length];
            reader.read_exact(&mut body).unwrap();
            let request: Value = serde_json::from_slice(&body).unwrap();
            let response =
                json!({"jsonrpc": "2.0", "id": request["id"], "result": {"tools": []}}).to_string();
            write!(reader.get_mut(), "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}", response.len()).unwrap();
            reader.get_mut().flush().unwrap();
        }
    });
    (url, handle)
}

#[test]
fn invalid_or_empty_pem_is_rejected_before_client_use() {
    for input in [
        b"".as_slice(),
        b"not PEM".as_slice(),
        b"-----BEGIN CERTIFICATE-----\n!bad-base64!\n-----END CERTIFICATE-----\n".as_slice(),
        b"-----BEGIN CERTIFICATE-----\nAA==\n-----END CERTIFICATE-----\n".as_slice(),
    ] {
        assert!(RootCertificates::from_pem(input).is_err());
    }
    let mut mixed = fixture::ROOT_1_PEM.to_vec();
    mixed.extend_from_slice(b"-----BEGIN CERTIFICATE-----\nAA==\n-----END CERTIFICATE-----\n");
    assert!(RootCertificates::from_pem(&mixed).is_err());
}

fn transport_result(roots: RootCertificates, host: &str) -> Result<Value> {
    let (url, handle) = https_server(fixture::LEAF_1_DER, fixture::LEAF_1_KEY_DER, host);
    let result = McpClient::with_root_certificates(url, None, roots)
        .unwrap()
        .list_tools(Some(Duration::from_secs(3)));
    handle.join().unwrap();
    result
}

#[test]
fn https_requires_correct_ca_and_hostname() {
    let absent = transport_result(RootCertificates::default(), "localhost").unwrap_err();
    assert!(format!("{absent:#}").contains("UnknownIssuer"));
    let wrong = transport_result(
        RootCertificates::from_pem(fixture::ROOT_2_PEM).unwrap(),
        "localhost",
    )
    .unwrap_err();
    assert!(format!("{wrong:#}").contains("UnknownIssuer"));
    let roots = RootCertificates::from_pem(fixture::ROOT_1_PEM).unwrap();
    let mismatch = transport_result(roots.clone(), "127.0.0.1").unwrap_err();
    let mismatch_chain = format!("{mismatch:#}");
    assert!(
        mismatch_chain.contains("not valid for name"),
        "unexpected hostname failure: {mismatch_chain}"
    );
    assert_eq!(
        transport_result(roots, "localhost").unwrap(),
        json!({"tools": []})
    );
}

#[test]
fn multiple_additional_anchors_work_through_both_gateway_constructors() {
    let mut pem = fixture::ROOT_1_PEM.to_vec();
    pem.extend_from_slice(fixture::ROOT_2_PEM);
    let roots = RootCertificates::from_pem(&pem).unwrap();
    for (leaf, key, local_test) in [
        (fixture::LEAF_1_DER, fixture::LEAF_1_KEY_DER, false),
        (fixture::LEAF_2_DER, fixture::LEAF_2_KEY_DER, true),
    ] {
        let (url, handle) = https_server(leaf, key, "localhost");
        let config = if local_test {
            ServerConfig::for_local_test("test", url)
        } else {
            ServerConfig::new("test", url)
        }
        .with_root_certificates(roots.clone());
        let base = std::env::temp_dir().join(format!(
            "trollm-root-test-{}-{local_test}",
            std::process::id()
        ));
        let gateway = McpGateway::new(
            vec![config],
            PolicySet::from_yaml_str("{}").unwrap(),
            base.join("cache"),
            base.join("artifacts"),
            None,
        )
        .unwrap();
        assert!(gateway.list_tools().unwrap().is_empty());
        handle.join().unwrap();
        drop(gateway);
        if base.exists() {
            std::fs::remove_dir_all(base).unwrap();
        }
    }
}
