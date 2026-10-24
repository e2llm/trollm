use std::{
    io::{Read, Write},
    net::TcpListener,
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

const BINARY: &str = env!("CARGO_BIN_EXE_trollm-flow-review");
struct Server(Child);
impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn probe(bind: &str) -> std::process::ExitStatus {
    Command::new(BINARY)
        .arg("--healthcheck")
        .env("FLOW_REVIEW_BIND", bind)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .unwrap()
}
#[test]
fn real_process_probe_and_graceful_sigterm() {
    let temp = tempfile::tempdir().unwrap();
    let reservation = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = reservation.local_addr().unwrap().port();
    drop(reservation);
    let bind = format!("0.0.0.0:{port}");
    let mut server = Server(
        Command::new(BINARY)
            .env("FLOW_REVIEW_BIND", &bind)
            .env("FLOW_REVIEW_PACKAGES", temp.path().join("packages"))
            .env("FLOW_REVIEW_TEMP", temp.path().join("staging"))
            .env("FLOW_REVIEW_STATIC", temp.path().join("web"))
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    while !probe(&bind).success() {
        assert!(Instant::now() < deadline, "server did not become healthy");
        assert!(
            server.0.try_wait().unwrap().is_none(),
            "server exited early"
        );
        thread::sleep(Duration::from_millis(25));
    }
    assert!(probe(&format!("localhost:{port}")).success());
    assert!(
        !temp.path().join("staging").exists(),
        "health checks must not stage imports"
    );
    #[cfg(unix)]
    {
        // The shell is a test harness only; the deployed probe uses the binary.
        assert!(Command::new("sh")
            .args([
                "-c",
                "kill -TERM \"$1\"",
                "signal-test",
                &server.0.id().to_string()
            ])
            .status()
            .unwrap()
            .success());
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(status) = server.0.try_wait().unwrap() {
                assert!(status.success(), "SIGTERM should finish graceful serving");
                break;
            }
            assert!(Instant::now() < deadline, "server ignored SIGTERM");
            thread::sleep(Duration::from_millis(25));
        }
        assert!(!probe(&bind).success());
    }
}
#[test]
fn real_process_probe_rejects_unhealthy_malformed_and_oversized_responses() {
    for response in [
        b"HTTP/1.1 503 Unavailable\r\nContent-Length: 15\r\n\r\n{\"status\":\"ok\"}".to_vec(),
        b"HTTP/1.1 200 OK\r\n\r\n{\"status\":\"bad\"}".to_vec(),
        b"HTTP/1.1 200 OK\r\n\r\nnot JSON".to_vec(),
        [
            b"HTTP/1.1 200 OK\r\n\r\n".as_slice(),
            &vec![b'x'; 17 * 1024],
        ]
        .concat(),
    ] {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let worker = thread::spawn(move || {
            let (mut client, _) = listener.accept().unwrap();
            client
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut buffer = [0; 1024];
            let _ = client.read(&mut buffer);
            let _ = client.write_all(&response);
        });
        assert!(!probe(&address.to_string()).success());
        worker.join().unwrap();
    }
    assert!(!probe("invalid bind").success());
}
#[test]
fn real_process_probe_bounds_a_stalled_endpoint() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let worker = thread::spawn(move || {
        let (_client, _) = listener.accept().unwrap();
        thread::sleep(Duration::from_secs(4));
    });
    let started = Instant::now();
    assert!(!probe(&address.to_string()).success());
    assert!(started.elapsed() < Duration::from_secs(5));
    worker.join().unwrap();
}

#[test]
fn real_process_ipv6_unspecified_probe_and_rejected_hostname_startup() {
    if let Ok(listener) = TcpListener::bind("[::1]:0") {
        let port = listener.local_addr().unwrap().port();
        let worker = thread::spawn(move || {
            let (mut client, _) = listener.accept().unwrap();
            client
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut buffer = [0; 1024];
            let _ = client.read(&mut buffer);
            client
                .write_all(b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n{\"status\":\"ok\"}")
                .unwrap();
        });
        assert!(probe(&format!("[::]:{port}")).success());
        worker.join().unwrap();
    }
    let output = Command::new(BINARY)
        .env("FLOW_REVIEW_BIND", "example.invalid:8080")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr)
        .contains("FLOW_REVIEW_BIND requires an IP socket address or localhost:port"));
}
