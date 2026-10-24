mod healthcheck;

use std::path::PathBuf;
use trollm_flow_review::http::{router, Config};
fn setting(name: &str, default: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| default.into())
}
fn main() -> anyhow::Result<()> {
    let arguments: Vec<_> = std::env::args().skip(1).collect();
    if arguments == ["--healthcheck"] {
        return healthcheck::check(&setting("FLOW_REVIEW_BIND", "0.0.0.0:8080"));
    }
    anyhow::ensure!(
        arguments.is_empty(),
        "Usage: trollm-flow-review [--healthcheck]"
    );
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(serve())
}
async fn serve() -> anyhow::Result<()> {
    let config = Config {
        packages: PathBuf::from(setting("FLOW_REVIEW_PACKAGES", "/packages")),
        temp: PathBuf::from(setting("FLOW_REVIEW_TEMP", "/tmp/trollm-flow-review")),
        static_dir: PathBuf::from(setting("FLOW_REVIEW_STATIC", "/app/web")),
    };
    #[cfg(unix)]
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    let address = healthcheck::bind_address(&setting("FLOW_REVIEW_BIND", "0.0.0.0:8080"))?;
    let listener = tokio::net::TcpListener::bind(address).await?;
    axum::serve(listener, router(config))
        .with_graceful_shutdown(async move {
            #[cfg(unix)]
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {},
                _ = terminate.recv() => {},
            }
            #[cfg(not(unix))]
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}
