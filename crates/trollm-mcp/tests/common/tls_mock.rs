//! TLS carrier for the existing shared HTTP mock. No production sender or
//! business verifier; the original mock alone counts decoded MCP requests.
use crate::common::{tls_fixture, MockMcpServer};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::thread::JoinHandle;

pub struct TlsMcpServer {
    pub url: String,
    inner: MockMcpServer,
    _thread: JoinHandle<()>,
}
impl TlsMcpServer {
    pub fn start() -> Self {
        Self::wrap(MockMcpServer::start())
    }
    pub fn start_with_protocol(version: &'static str) -> Self {
        Self::wrap(MockMcpServer::start_with_protocol(version))
    }
    fn wrap(inner: MockMcpServer) -> Self {
        let target = inner
            .url
            .strip_prefix("http://")
            .unwrap()
            .strip_suffix("/mcp")
            .unwrap()
            .to_owned();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let thread = std::thread::spawn(move || {
            let config = rustls::ServerConfig::builder_with_provider(Arc::new(
                rustls::crypto::ring::default_provider(),
            ))
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(
                vec![rustls::pki_types::CertificateDer::from(
                    tls_fixture::LEAF_1_DER.to_vec(),
                )],
                rustls::pki_types::PrivateKeyDer::Pkcs8(
                    rustls::pki_types::PrivatePkcs8KeyDer::from(
                        tls_fixture::LEAF_1_KEY_DER.to_vec(),
                    ),
                ),
            )
            .unwrap();
            let config = Arc::new(config);
            for socket in listener.incoming() {
                let Ok(socket) = socket else {
                    break;
                };
                socket
                    .set_read_timeout(Some(std::time::Duration::from_secs(3)))
                    .unwrap();
                socket
                    .set_write_timeout(Some(std::time::Duration::from_secs(3)))
                    .unwrap();
                let mut stream = rustls::StreamOwned::new(
                    rustls::ServerConnection::new(config.clone()).unwrap(),
                    socket,
                );
                let mut reader = BufReader::new(&mut stream);
                let mut head = Vec::new();
                let mut length = 0;
                loop {
                    let mut line = String::new();
                    if !matches!(reader.read_line(&mut line), Ok(n) if n > 0) {
                        break;
                    }
                    head.extend_from_slice(line.as_bytes());
                    if line == "\r\n" {
                        break;
                    }
                    if let Some((name, value)) = line.split_once(':') {
                        if name.eq_ignore_ascii_case("content-length") {
                            length = value.trim().parse::<usize>().unwrap();
                        }
                    }
                    assert!(head.len() <= 32768);
                }
                if !head.ends_with(b"\r\n\r\n") {
                    continue;
                }
                assert!(length <= 1024 * 1024);
                let mut body = vec![0; length];
                if reader.read_exact(&mut body).is_err() {
                    continue;
                }
                drop(reader);
                let mut upstream = TcpStream::connect(&target).unwrap();
                upstream
                    .set_read_timeout(Some(std::time::Duration::from_secs(3)))
                    .unwrap();
                upstream.write_all(&head).unwrap();
                upstream.write_all(&body).unwrap();
                let mut reply = Vec::new();
                upstream.take(1024 * 1024).read_to_end(&mut reply).unwrap();
                let _ = stream.write_all(&reply);
                let _ = stream.flush();
            }
        });
        Self {
            url: format!("https://localhost:{port}/mcp"),
            inner,
            _thread: thread,
        }
    }
    pub fn count(&self, name: &str) -> usize {
        self.inner.count(name)
    }
}
