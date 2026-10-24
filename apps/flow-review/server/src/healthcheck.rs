//! A local process probe for shell-free container runtimes. No application
//! state, imports, model code or async runtime is initialized in this mode.
use anyhow::{bail, Context, Result};
use std::{
    io::{Read, Write},
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpStream},
    time::{Duration, Instant},
};

const TIMEOUT: Duration = Duration::from_secs(3);
const MAX_RESPONSE: usize = 16 * 1024;

pub fn bind_address(bind: &str) -> Result<SocketAddr> {
    // Bind is normally a numeric socket address. Also support the conventional
    // localhost alias without an unbounded DNS lookup in a readiness probe.
    let address: SocketAddr = if let Some(port) = bind.strip_prefix("localhost:") {
        SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port.parse()?)
    } else {
        bind.parse()
            .context("FLOW_REVIEW_BIND requires an IP socket address or localhost:port")?
    };
    Ok(address)
}

pub fn check(bind: &str) -> Result<()> {
    let mut address = bind_address(bind)?;
    if address.ip().is_unspecified() {
        address.set_ip(match address.ip() {
            IpAddr::V4(_) => IpAddr::V4(Ipv4Addr::LOCALHOST),
            IpAddr::V6(_) => IpAddr::V6(Ipv6Addr::LOCALHOST),
        });
    }
    let deadline = Instant::now() + TIMEOUT;
    let mut stream = TcpStream::connect_timeout(&address, Duration::from_secs(1))
        .context("health endpoint connection failed")?;
    stream.set_write_timeout(Some(Duration::from_secs(1)))?;
    stream.write_all(
        format!("GET /healthz HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\n\r\n").as_bytes(),
    )?;
    let mut response = Vec::new();
    loop {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .context("health endpoint timed out")?;
        stream.set_read_timeout(Some(remaining))?;
        let mut buffer = [0; 1024];
        let size = stream.read(&mut buffer)?;
        if size == 0 {
            break;
        }
        if response.len() + size > MAX_RESPONSE {
            bail!("health endpoint response exceeds limit");
        }
        response.extend_from_slice(&buffer[..size]);
    }
    let split = response
        .windows(4)
        .position(|bytes| bytes == b"\r\n\r\n")
        .context("invalid health endpoint response")?;
    let headers = std::str::from_utf8(&response[..split])?;
    let mut status = headers
        .lines()
        .next()
        .context("missing health status")?
        .split_whitespace();
    if !matches!(status.next(), Some("HTTP/1.1" | "HTTP/1.0")) || status.next() != Some("200") {
        bail!("health endpoint did not return HTTP 200");
    }
    let value: serde_json::Value =
        serde_json::from_slice(&response[split + 4..]).context("invalid health endpoint JSON")?;
    if value.get("status").and_then(serde_json::Value::as_str) != Some("ok") {
        bail!("health endpoint did not report ok");
    }
    Ok(())
}
