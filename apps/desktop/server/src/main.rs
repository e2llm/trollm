use std::io::{BufRead, BufReader, Read, Write};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpStream};
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{ensure, Context};
use trollm_desktop::{router, Host, Origins};

const DEFAULT_ORIGIN: &str = "http://127.0.0.1:8080";

fn setting(name: &str, default: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| default.to_owned())
}

fn data_dir() -> anyhow::Result<PathBuf> {
    if let Some(path) = std::env::var_os("TROLLM_DESKTOP_DATA") {
        return Ok(path.into());
    }
    let data_home = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share")))
        .context("set TROLLM_DESKTOP_DATA to the local data directory")?;
    Ok(data_home.join("trollm-desktop"))
}

fn main() -> anyhow::Result<()> {
    let arguments = std::env::args().skip(1).collect::<Vec<_>>();
    ensure!(
        arguments.is_empty() || arguments == ["--healthcheck"],
        "Usage: trollm-desktop [--healthcheck]"
    );
    let address = setting("TROLLM_DESKTOP_BIND", "127.0.0.1:8080")
        .parse::<SocketAddr>()
        .context("invalid desktop bind address")?;
    let origins = Origins::new(
        &setting("TROLLM_DESKTOP_ORIGIN", DEFAULT_ORIGIN),
        std::env::var("TROLLM_DESKTOP_ORIGIN_ALIAS").ok().as_deref(),
    )?;
    if arguments == ["--healthcheck"] {
        return healthcheck(address, &origins);
    }

    let static_dir = PathBuf::from(setting("TROLLM_DESKTOP_WEB", "apps/desktop/web/dist"))
        .canonicalize()
        .context("desktop web build is unavailable")?;
    ensure!(
        static_dir.join("index.html").is_file(),
        "desktop web build is unavailable"
    );
    let starters = PathBuf::from(setting(
        "TROLLM_DESKTOP_STARTERS",
        "apps/android/app/src/main/assets/flows",
    ));
    let data = data_dir()?;
    let host = Host::open(&data, &starters, origins.callback_url())?;
    let data = data.canonicalize()?;
    ensure!(
        !data.starts_with(&static_dir) && !static_dir.starts_with(&data),
        "desktop web assets and private data must use separate directories"
    );
    let app = router(host, origins, static_dir);
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(serve(address, app))
}

async fn serve(address: SocketAddr, app: axum::Router) -> anyhow::Result<()> {
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    let listener = tokio::net::TcpListener::bind(address).await?;
    eprintln!("trollm desktop listening on {}", listener.local_addr()?);
    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {},
                _ = terminate.recv() => {},
            }
        })
        .await?;
    Ok(())
}

fn healthcheck(mut address: SocketAddr, origins: &Origins) -> anyhow::Result<()> {
    address.set_ip(match address.ip() {
        IpAddr::V4(ip) if ip.is_unspecified() => Ipv4Addr::LOCALHOST.into(),
        IpAddr::V6(ip) if ip.is_unspecified() => Ipv6Addr::LOCALHOST.into(),
        ip => ip,
    });
    let timeout = Duration::from_secs(2);
    let mut stream = TcpStream::connect_timeout(&address, timeout)?;
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(timeout))?;
    write!(
        stream,
        "GET /healthz HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n",
        origins.authority()
    )?;
    let mut result = String::new();
    BufReader::new(stream.take(1024)).read_line(&mut result)?;
    ensure!(
        result.starts_with("HTTP/1.1 200 "),
        "desktop health endpoint did not report success"
    );
    Ok(())
}
