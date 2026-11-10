//! Selected-profile HTTP/1.1 write boundary. One fresh connection, no pool,
//! redirect, proxy fallback or automatic retry. The final check precedes the
//! first TLS plaintext acceptance, including HTTP headers, after readiness.
use anyhow::{ensure, Context, Result};
use std::io;
#[cfg(test)]
use std::io::{Read, Write};
use std::net::ToSocketAddrs;
#[cfg(test)]
use std::os::fd::AsRawFd;
use std::sync::Arc;
use std::time::{Duration, Instant};

const MAX_BODY: usize = 32 * 1024 * 1024;
const MAX_HEAD: usize = 32 * 1024;

pub(crate) struct Response {
    pub status: u16,
    pub headers: reqwest::header::HeaderMap,
    pub bytes: Vec<u8>,
}

// An explicit standard HTTPS port is valid authority syntax. Keep it and fix
// incompatible routing at its owner, rather than silently changing endpoint policy.
fn selected_authority(host: &str, port: u16) -> String {
    format!("{host}:{port}")
}

/// Guard caller-owned plaintext IO, before rustls can buffer any application
/// bytes. A delegated error is ambiguous even when no count was returned.
struct PlaintextIo<'a> {
    tls: tokio_rustls::client::TlsStream<tokio::net::TcpStream>,
    gate: &'a dyn Fn() -> Result<()>,
    deadline: Instant,
    possible_send: bool,
}
impl tokio::io::AsyncRead for PlaintextIo<'_> {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        output: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        std::pin::Pin::new(&mut self.tls).poll_read(cx, output)
    }
}
impl tokio::io::AsyncWrite for PlaintextIo<'_> {
    fn poll_write(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        input: &[u8],
    ) -> std::task::Poll<io::Result<usize>> {
        use std::task::Poll;
        if input.is_empty() {
            return Poll::Ready(Ok(0));
        }
        if !self.possible_send {
            // Readiness must precede admission. Pending/zero acceptance does not
            // latch an allow: the next poll repeats readiness and admission.
            std::task::ready!(self.tls.get_ref().0.poll_write_ready(cx))?;
            if let Err(error) = (self.gate)() {
                return Poll::Ready(Err(io::Error::other(error)));
            }
        }
        if Instant::now() >= self.deadline {
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "selected plaintext deadline",
            )));
        }
        let result = std::pin::Pin::new(&mut self.tls).poll_write(cx, input);
        if matches!(&result, Poll::Ready(Ok(count)) if *count > 0)
            || matches!(&result, Poll::Ready(Err(_)))
        {
            self.possible_send = true;
        }
        result
    }
    // Default vectored implementation selects a nonempty slice and calls our
    // poll_write, so no alternate header/body acceptance path bypasses the gate.
    fn poll_flush(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        std::pin::Pin::new(&mut self.tls).poll_flush(cx)
    }
    fn poll_shutdown(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        std::pin::Pin::new(&mut self.tls).poll_shutdown(cx)
    }
}

#[cfg(test)]
pub(crate) fn send(
    request: reqwest::blocking::Request,
    roots: &super::RootCertificates,
    timeout: Duration,
    gate: &dyn Fn() -> Result<()>,
) -> Result<Response> {
    send_acquired(request, roots, timeout, &|| Ok(()), gate)
}

pub(crate) fn send_acquired(
    request: reqwest::blocking::Request,
    roots: &super::RootCertificates,
    timeout: Duration,
    acquire: &dyn Fn() -> Result<()>,
    gate: &dyn Fn() -> Result<()>,
) -> Result<Response> {
    use http_body_util::BodyExt;
    use tokio::io::AsyncWriteExt;
    ensure!(
        request.method() == reqwest::Method::POST,
        "selected transport requires POST"
    );
    let url = request.url();
    ensure!(
        url.scheme() == "https"
            && url.username().is_empty()
            && url.password().is_none()
            && url.fragment().is_none(),
        "selected write requires an exact HTTPS endpoint"
    );
    let deadline = Instant::now()
        .checked_add(timeout)
        .context("selected deadline overflow")?;
    let host = url
        .host_str()
        .context("selected endpoint host unavailable")?;
    let port = url
        .port_or_known_default()
        .context("selected endpoint port unavailable")?;
    let dns_host = host.trim_start_matches('[').trim_end_matches(']');
    let body = request
        .body()
        .and_then(|body| body.as_bytes())
        .context("selected request body unavailable")?;
    ensure!(body.len() <= 69632, "selected request exceeds byte bound");
    let mut head_bytes = 0usize;
    for (name, value) in request.headers() {
        ensure!(
            !matches!(
                name.as_str(),
                "host"
                    | "connection"
                    | "content-length"
                    | "transfer-encoding"
                    | "expect"
                    | "upgrade"
            ),
            "selected request contains unsupported transport override"
        );
        head_bytes = head_bytes.saturating_add(name.as_str().len() + value.as_bytes().len() + 4);
    }
    ensure!(
        head_bytes.saturating_add(url.as_str().len() + 256) <= MAX_HEAD,
        "selected request headers exceed bound"
    );
    // DNS may block beyond cooperative timeout, but cannot emit application data.
    let addresses: Vec<_> = (dns_host, port).to_socket_addrs()?.take(16).collect();
    let address = *addresses
        .iter()
        .find(|a| a.is_ipv4())
        .or_else(|| addresses.first())
        .context("selected endpoint did not resolve")?;
    let config = roots.selected_tls_config()?;
    let name = rustls::pki_types::ServerName::try_from(dns_host.to_owned())?;
    let mut path = url.path().to_owned();
    if let Some(query) = url.query() {
        path.push('?');
        path.push_str(query);
    }
    let mut outbound = hyper::Request::builder()
        .method("POST")
        .uri(path)
        .version(hyper::Version::HTTP_11)
        .header("Host", selected_authority(host, port))
        .header("Connection", "close");
    for (name, value) in request.headers() {
        outbound = outbound.header(name, value);
    }
    let outbound = outbound.body(http_body_util::Full::new(bytes::Bytes::copy_from_slice(
        body,
    )))?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let tls = runtime.block_on(async {
        tokio::time::timeout(deadline.saturating_duration_since(Instant::now()), async {
            // One address/connection only. No pool, proxy, redirect or retry.
            let stream = tokio::net::TcpStream::connect(address).await?;
            stream.set_nodelay(true)?;
            let mut tls = tokio_rustls::TlsConnector::from(Arc::new(config))
                .connect(name, stream)
                .await
                .map_err(|error| {
                    anyhow::anyhow!("selected TLS handshake failed ({:?})", error.kind())
                })?;
            tls.flush().await?;
            ensure!(
                tls.get_ref().1.alpn_protocol().is_none()
                    || tls.get_ref().1.alpn_protocol() == Some(b"http/1.1"),
                "selected transport negotiated unsupported protocol"
            );
            Ok::<_, anyhow::Error>(tls)
        })
        .await
        .context("selected TLS readiness deadline")?
    })?;
    // The acquisition uses existing blocking clients outside the async runtime.
    // It may consume time, but cannot renew the original transport deadline.
    acquire()?;
    // Retain only bounded private response evidence after actual headers arrive.
    // A timeout before headers does not invent a status or response body.
    let capture = std::sync::Mutex::new(None::<crate::transport::HttpResponseCapture>);
    let outcome = runtime.block_on(async {
        let work = async {
            let io = hyper_util::rt::TokioIo::new(PlaintextIo {
                tls,
                gate,
                deadline,
                possible_send: false,
            });
            let (mut sender, connection) = hyper::client::conn::http1::Builder::new()
                .max_headers(64)
                .max_buf_size(MAX_HEAD)
                .handshake(io)
                .await?;
            let receive = async {
                let response = sender
                    .send_request(outbound)
                    .await
                    .context("selected request failed; possible send")?;
                drop(sender);
                *capture.lock().unwrap() = Some(crate::transport::HttpResponseCapture::new(
                    response.status().as_u16(),
                    response.headers(),
                ));
                ensure!(
                    response.version() == hyper::Version::HTTP_11
                        && response.status().as_u16() >= 200,
                    "selected response protocol unsupported"
                );
                let status = response.status().as_u16();
                let headers = response.headers().clone();
                if let Some(length) = headers.get("content-length") {
                    ensure!(
                        length.to_str()?.parse::<u64>()? <= MAX_BODY as u64,
                        "selected response exceeds byte limit"
                    );
                }
                for name in headers.keys() {
                    ensure!(
                        headers.get_all(name).iter().count() == 1,
                        "selected duplicate response header refused"
                    );
                }
                ensure!(
                    !(headers.contains_key("transfer-encoding")
                        && headers.contains_key("content-length")),
                    "selected ambiguous response framing"
                );
                let mut body = response.into_body();
                let mut bytes = Vec::new();
                while let Some(frame) = body.frame().await {
                    let frame = frame.context("selected response failed; possible send")?;
                    ensure!(
                        !frame.is_trailers(),
                        "selected response trailers unsupported"
                    );
                    if let Ok(data) = frame.into_data() {
                        capture.lock().unwrap().as_mut().unwrap().observe(&data);
                        ensure!(
                            data.len() <= MAX_BODY.saturating_sub(bytes.len()),
                            "selected response exceeds byte limit"
                        );
                        bytes.extend_from_slice(&data);
                    }
                }
                Ok::<_, anyhow::Error>(Response {
                    status,
                    headers,
                    bytes,
                })
            };
            let (response, ()) = tokio::try_join!(receive, async {
                connection
                    .await
                    .context("selected connection failed; possible send")
            })?;
            Ok::<_, anyhow::Error>(response)
        };
        tokio::time::timeout(deadline.saturating_duration_since(Instant::now()), work)
            .await
            .context("selected request deadline; possible send")?
    });
    outcome.map_err(|error| {
        if let Some(observed) = capture.lock().unwrap().as_ref() {
            let cause = if error
                .downcast_ref::<tokio::time::error::Elapsed>()
                .is_some()
            {
                "deadline"
            } else {
                "response-read-or-framing"
            };
            return observed.failure(cause, false).into();
        }
        error
    })
}

#[cfg(test)]
mod controls {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    #[allow(dead_code)]
    mod tls {
        include!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/common/tls_fixture.rs"
        ));
    }

    #[test]
    fn delayed_tls_readiness_rechecks_before_headers_and_exact_success_sends_once() {
        for mode in 0..6 {
            let refuse = mode != 0;
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let port = listener.local_addr().unwrap().port();
            let canceled = Arc::new(AtomicBool::new(false));
            let remote_cancel = canceled.clone();
            let peer = std::thread::spawn(move || {
                let (socket, _) = listener.accept().unwrap();
                socket
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                socket
                    .set_write_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                // Real TCP is already connected; defer real TLS readiness.
                std::thread::sleep(Duration::from_millis(70));
                if mode == 1 {
                    remote_cancel.store(true, Ordering::SeqCst);
                }
                let config = rustls::ServerConfig::builder_with_provider(Arc::new(
                    rustls::crypto::ring::default_provider(),
                ))
                .with_safe_default_protocol_versions()
                .unwrap()
                .with_no_client_auth()
                .with_single_cert(
                    vec![rustls::pki_types::CertificateDer::from(
                        tls::LEAF_1_DER.to_vec(),
                    )],
                    rustls::pki_types::PrivateKeyDer::Pkcs8(
                        rustls::pki_types::PrivatePkcs8KeyDer::from(tls::LEAF_1_KEY_DER.to_vec()),
                    ),
                )
                .unwrap();
                let connection = rustls::ServerConnection::new(Arc::new(config)).unwrap();
                let mut stream = rustls::StreamOwned::new(connection, socket);
                let mut received = Vec::new();
                let mut byte = [0];
                while let Ok(1) = stream.read(&mut byte) {
                    received.push(byte[0]);
                    if received.ends_with(b"\r\n\r\n") {
                        break;
                    }
                    assert!(received.len() <= MAX_HEAD);
                }
                if !received.is_empty() {
                    assert!(received.starts_with(b"POST /mcp HTTP/1.1\r\n"));
                    assert!(String::from_utf8_lossy(&received)
                        .contains("authorization: Bearer fixture-only-key"));
                    let head = String::from_utf8_lossy(&received);
                    let length: usize = head
                        .lines()
                        .find_map(|line| {
                            line.split_once(':')
                                .filter(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                                .map(|(_, value)| value.trim().parse().unwrap())
                        })
                        .unwrap();
                    let mut body = vec![0; length];
                    stream.read_exact(&mut body).unwrap();
                    assert_eq!(
                        serde_json::from_slice::<serde_json::Value>(&body).unwrap()["blob"]
                            .as_str()
                            .unwrap()
                            .len(),
                        40000
                    );
                    received.extend_from_slice(&body);
                    stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}").unwrap();
                    stream.flush().unwrap();
                }
                received
            });
            let request = reqwest::blocking::Client::new()
                .post(format!(
                    "https://{}:{port}/mcp",
                    if mode == 4 { "127.0.0.1" } else { "localhost" }
                ))
                .bearer_auth("fixture-only-key")
                .json(&if mode == 0 {
                    serde_json::json!({"blob":"x".repeat(40000)})
                } else {
                    serde_json::json!({})
                })
                .build()
                .unwrap();
            let exact_body = request.body().unwrap().as_bytes().unwrap().to_vec();
            let checks = AtomicUsize::new(0);
            let start = Instant::now();
            let result = send_acquired(
                request,
                &super::super::RootCertificates::from_pem(if mode == 3 {
                    tls::ROOT_2_PEM
                } else {
                    tls::ROOT_1_PEM
                })
                .unwrap(),
                Duration::from_secs(2),
                &|| {
                    if mode == 5 {
                        std::thread::sleep(Duration::from_millis(30));
                        canceled.store(true, Ordering::SeqCst);
                    }
                    Ok(())
                },
                &|| {
                    checks.fetch_add(1, Ordering::SeqCst);
                    ensure!(
                        start.elapsed() >= Duration::from_millis(60),
                        "gate ran before TLS readiness"
                    );
                    ensure!(
                        mode != 2 || start.elapsed() < Duration::from_millis(40),
                        "controlled expiry after TLS readiness"
                    );
                    ensure!(
                        !canceled.load(Ordering::SeqCst),
                        "controlled cancellation after readiness wait"
                    );
                    Ok(())
                },
            );
            let received = peer.join().unwrap();
            assert_eq!(result.is_ok(), !refuse);
            assert_eq!(
                received.is_empty(),
                refuse,
                "refusal must withhold headers, not just body"
            );
            if mode == 3 || mode == 4 {
                assert_eq!(
                    checks.load(Ordering::SeqCst),
                    0,
                    "untrusted TLS must fail before the gate"
                );
            } else {
                assert!(checks.load(Ordering::SeqCst) > 0);
            }
            if let Ok(response) = result {
                assert_eq!(response.bytes, b"{}");
                assert!(
                    received.ends_with(&exact_body),
                    "serialized request body changed"
                );
            }
        }
    }

    #[test]
    fn plaintext_backpressure_rechecks_after_wait_before_tls_acceptance() {
        use tokio::io::AsyncWriteExt;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let size: libc::c_int = 4096;
        assert_eq!(
            unsafe {
                libc::setsockopt(
                    listener.as_raw_fd(),
                    libc::SOL_SOCKET,
                    libc::SO_RCVBUF,
                    (&size as *const libc::c_int).cast(),
                    std::mem::size_of_val(&size) as libc::socklen_t,
                )
            },
            0
        );
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let (fill_tx, fill_rx) = std::sync::mpsc::channel();
        let canceled = Arc::new(AtomicBool::new(false));
        let remote_cancel = canceled.clone();
        let peer = std::thread::spawn(move || {
            let (socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let config = rustls::ServerConfig::builder_with_provider(Arc::new(
                rustls::crypto::ring::default_provider(),
            ))
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(
                vec![rustls::pki_types::CertificateDer::from(
                    tls::LEAF_1_DER.to_vec(),
                )],
                rustls::pki_types::PrivateKeyDer::Pkcs8(
                    rustls::pki_types::PrivatePkcs8KeyDer::from(tls::LEAF_1_KEY_DER.to_vec()),
                ),
            )
            .unwrap();
            let mut connection = rustls::ServerConnection::new(Arc::new(config)).unwrap();
            let mut socket = socket;
            while connection.is_handshaking() {
                connection.complete_io(&mut socket).unwrap();
            }
            while connection.wants_write() {
                connection.write_tls(&mut socket).unwrap();
            }
            ready_tx.send(()).unwrap();
            fill_rx.recv().unwrap();
            std::thread::sleep(Duration::from_millis(60));
            remote_cancel.store(true, Ordering::SeqCst);
            // Raw prefill is test-only backpressure, not application TLS data.
            let mut received = Vec::new();
            if let Err(error) = socket.read_to_end(&mut received) {
                assert_eq!(error.kind(), io::ErrorKind::ConnectionReset);
            }
            received
        });
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let tls = runtime.block_on(async {
            let stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
                .await
                .unwrap();
            let config = super::super::RootCertificates::from_pem(tls::ROOT_1_PEM)
                .unwrap()
                .selected_tls_config()
                .unwrap();
            let mut tls = tokio_rustls::TlsConnector::from(Arc::new(config))
                .connect(
                    rustls::pki_types::ServerName::try_from("localhost").unwrap(),
                    stream,
                )
                .await
                .unwrap();
            tls.flush().await.unwrap();
            tls
        });
        ready_rx.recv().unwrap();
        let stream = tls.get_ref().0;
        assert_eq!(
            unsafe {
                libc::setsockopt(
                    stream.as_raw_fd(),
                    libc::SOL_SOCKET,
                    libc::SO_SNDBUF,
                    (&size as *const libc::c_int).cast(),
                    std::mem::size_of_val(&size) as libc::socklen_t,
                )
            },
            0
        );
        let mut filled = 0;
        loop {
            match stream.try_write(&[b'x'; 65536]) {
                Ok(n) => {
                    filled += n;
                    assert!(filled < 16 * 1024 * 1024);
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    let mut probe = libc::pollfd {
                        fd: stream.as_raw_fd(),
                        events: libc::POLLOUT,
                        revents: 0,
                    };
                    if unsafe { libc::poll(&mut probe, 1, 30) } == 0 {
                        break;
                    }
                }
                other => panic!("unexpected prefill {other:?}"),
            }
        }
        fill_tx.send(()).unwrap();
        let gate = || {
            ensure!(
                !canceled.load(Ordering::SeqCst),
                "controlled canceled plaintext"
            );
            Ok(())
        };
        let mut io = PlaintextIo {
            tls,
            gate: &gate,
            deadline: Instant::now() + Duration::from_secs(2),
            possible_send: false,
        };
        let waiting = Instant::now();
        assert!(runtime
            .block_on(io.write_all(b"POST /effect HTTP/1.1\r\n"))
            .is_err());
        assert!(waiting.elapsed() >= Duration::from_millis(50));
        assert!(!io.possible_send, "refused before TLS plaintext acceptance");
        drop(io);
        assert_eq!(peer.join().unwrap(), vec![b'x'; filled]);
    }

    #[test]
    fn credential_change_during_real_tls_wait_refuses_before_request_headers() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let changed = Arc::new(AtomicBool::new(false));
        let peer_changed = changed.clone();
        let peer = std::thread::spawn(move || {
            let config = rustls::ServerConfig::builder_with_provider(Arc::new(
                rustls::crypto::ring::default_provider(),
            ))
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(
                vec![rustls::pki_types::CertificateDer::from(
                    tls::LEAF_1_DER.to_vec(),
                )],
                rustls::pki_types::PrivateKeyDer::Pkcs8(
                    rustls::pki_types::PrivatePkcs8KeyDer::from(tls::LEAF_1_KEY_DER.to_vec()),
                ),
            )
            .unwrap();
            let config = Arc::new(config);
            let mut requests = Vec::new();
            for turn in 0..3 {
                let (socket, _) = listener.accept().unwrap();
                socket
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                socket
                    .set_write_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                if turn == 2 {
                    std::thread::sleep(Duration::from_millis(60));
                    peer_changed.store(true, Ordering::SeqCst);
                }
                let connection = rustls::ServerConnection::new(config.clone()).unwrap();
                let mut stream = rustls::StreamOwned::new(connection, socket);
                let mut head = Vec::new();
                let mut byte = [0];
                while let Ok(1) = stream.read(&mut byte) {
                    head.push(byte[0]);
                    assert!(head.len() <= MAX_HEAD);
                    if head.ends_with(b"\r\n\r\n") {
                        break;
                    }
                }
                if turn == 2 {
                    requests.push(head);
                    break;
                }
                let text = String::from_utf8(head.clone()).unwrap();
                assert!(text
                    .to_ascii_lowercase()
                    .contains("authorization: bearer fixture-old"));
                let length: usize = text
                    .lines()
                    .find_map(|line| {
                        line.split_once(':')
                            .filter(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                            .map(|(_, value)| value.trim().parse().unwrap())
                    })
                    .unwrap();
                let mut body = vec![0; length];
                stream.read_exact(&mut body).unwrap();
                let rpc: serde_json::Value = serde_json::from_slice(&body).unwrap();
                requests.push(head);
                let reply = if turn == 0 {
                    assert_eq!(rpc["method"], "initialize");
                    serde_json::json!({"jsonrpc":"2.0","id":rpc["id"],"result":{
                        "protocolVersion":"2025-06-18","capabilities":{"tools":{}},
                        "serverInfo":{"name":"controlled-transport","version":"1"}}})
                    .to_string()
                } else {
                    assert_eq!(rpc["method"], "notifications/initialized");
                    String::new()
                };
                write!(stream, "HTTP/1.1 {} OK\r\nContent-Type: application/json\r\nMcp-Session-Id: fixture-session\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    if turn == 0 {200} else {202}, reply.len(), reply).unwrap();
                stream.flush().unwrap();
            }
            requests
        });
        let token: super::super::transport::TokenGetter = Arc::new(move || {
            Some(if changed.load(Ordering::SeqCst) {
                "fixture-new".into()
            } else {
                "fixture-old".into()
            })
        });
        let mut client = super::super::transport::McpClient::with_root_certificates(
            format!("https://localhost:{port}/mcp"),
            Some(token),
            super::super::RootCertificates::from_pem(tls::ROOT_1_PEM).unwrap(),
        )
        .unwrap()
        .with_protocol_version("2025-06-18")
        .unwrap();
        let error = client
            .call_prepared_tool_checked(
                "act",
                &serde_json::json!({"action":"click"}),
                Some(Duration::from_secs(2)),
                &|| Ok(()),
            )
            .unwrap_err();
        assert!(format!("{error:#}").contains("credential changed"));
        let requests = peer.join().unwrap();
        assert_eq!(requests.len(), 3);
        assert!(
            requests[2].is_empty(),
            "rotated credential must not release old headers or body"
        );
    }

    fn response_fixture(reply: &'static [u8]) -> Result<Response> {
        response_fixture_with_delay(reply, Duration::ZERO, Duration::from_secs(2))
    }

    fn response_fixture_with_delay(
        reply: &'static [u8],
        delay: Duration,
        timeout: Duration,
    ) -> Result<Response> {
        let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
        let port = listener.local_addr()?.port();
        let peer = std::thread::spawn(move || {
            let (socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let config = rustls::ServerConfig::builder_with_provider(Arc::new(
                rustls::crypto::ring::default_provider(),
            ))
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(
                vec![rustls::pki_types::CertificateDer::from(
                    tls::LEAF_1_DER.to_vec(),
                )],
                rustls::pki_types::PrivateKeyDer::Pkcs8(
                    rustls::pki_types::PrivatePkcs8KeyDer::from(tls::LEAF_1_KEY_DER.to_vec()),
                ),
            )
            .unwrap();
            let mut stream = rustls::StreamOwned::new(
                rustls::ServerConnection::new(Arc::new(config)).unwrap(),
                socket,
            );
            let mut received = Vec::new();
            let mut byte = [0];
            while stream.read_exact(&mut byte).is_ok() {
                received.push(byte[0]);
                if received.ends_with(b"\r\n\r\n") {
                    break;
                }
            }
            let head = String::from_utf8(received.clone()).unwrap();
            assert!(head.starts_with("POST /mcp?declared=1 HTTP/1.1\r\n"));
            assert!(head
                .to_ascii_lowercase()
                .contains(&format!("host: localhost:{port}\r\n")));
            // Consume the actual two-byte request body before responding.
            stream.read_exact(&mut [0; 2]).unwrap();
            if !delay.is_zero() {
                let split = reply.windows(4).position(|v| v == b"\r\n\r\n").unwrap() + 5;
                let _ = stream.write_all(&reply[..split]);
                let _ = stream.flush();
                std::thread::sleep(delay);
                let _ = stream.write_all(&reply[split..]);
            } else {
                let _ = stream.write_all(reply);
            }
            let _ = stream.flush();
            stream.conn.send_close_notify();
            let _ = stream.flush();
        });
        let request = reqwest::blocking::Client::new()
            .post(format!("https://localhost:{port}/mcp?declared=1"))
            .body("{}")
            .build()?;
        let result = send(
            request,
            &super::super::RootCertificates::from_pem(tls::ROOT_1_PEM)?,
            timeout,
            &|| Ok(()),
        );
        peer.join().unwrap();
        result
    }
    #[test]
    fn maintained_http_response_framing_bounds_partial_and_lost_replies() {
        assert_eq!(response_fixture(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n2\r\n{}\r\n0\r\n\r\n").unwrap().bytes, b"{}");
        for bad in [
            &b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\ncontent-length: 3\r\n\r\n{}"[..],
            &b"HTTP/1.1 200 OK\r\nContent-Length: 33554433\r\n\r\n"[..],
            &b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{"[..],
            &b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\nZZ\r\n"[..],
            &b""[..],
        ] {
            assert!(response_fixture(bad).is_err());
        }
    }
    #[test]
    fn response_originals_survive_partial_framing_and_deadline_without_ack() {
        for bad in [
            &b"HTTP/1.1 421 Misdirected Request\r\nContent-Length: 9\r\n\r\nx"[..],
            &b"HTTP/1.1 421 Misdirected Request\r\nContent-Length: 33554433\r\n\r\n"[..],
        ] {
            let error = response_fixture(bad).err().unwrap();
            let observed = error
                .downcast_ref::<crate::transport::HttpResponseFailure>()
                .unwrap();
            assert_eq!(observed.status, 421);
            assert_eq!(observed.private["bodyComplete"], false);
            assert!(observed.private["bodySha256"].is_null());
        }
        let error = response_fixture_with_delay(
            b"HTTP/1.1 421 Misdirected Request\r\nContent-Length: 4\r\n\r\nlate",
            Duration::from_millis(200),
            Duration::from_millis(80),
        )
        .err()
        .unwrap();
        let observed = error
            .downcast_ref::<crate::transport::HttpResponseFailure>()
            .unwrap();
        assert_eq!(observed.status, 421);
        assert_eq!(observed.cause, "deadline");
        assert_eq!(observed.private["bodyComplete"], false);
        assert!(observed.private["bodySha256"].is_null());
    }

    #[test]
    fn endpoint_authority_retains_default_nondefault_and_ipv6_ports_and_rejects_overrides() {
        assert_eq!(selected_authority("api.example", 443), "api.example:443");
        assert_eq!(selected_authority("api.example", 8443), "api.example:8443");
        assert_eq!(selected_authority("[::1]", 443), "[::1]:443");
        for name in [
            "Host",
            "Connection",
            "Content-Length",
            "Transfer-Encoding",
            "Expect",
            "Upgrade",
        ] {
            let request = reqwest::blocking::Client::new()
                .post("https://localhost:1/mcp")
                .header(name, "forbidden-override")
                .body("{}")
                .build()
                .unwrap();
            let error = send(
                request,
                &super::super::RootCertificates::default(),
                Duration::from_millis(20),
                &|| Ok(()),
            )
            .err()
            .unwrap();
            assert!(error.to_string().contains("transport override"));
        }
    }
}
