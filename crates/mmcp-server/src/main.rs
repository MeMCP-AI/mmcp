//! mmcp server binary entry point.

use std::net::SocketAddr;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};

use mmcp_server::app;
use mmcp_server::config;
use mmcp_server::state;

#[derive(Parser)]
#[command(
    name = "mmcp-server",
    version,
    about = "mmcp server daemon: MCP transport, git smart-HTTP, auth, WebUI API"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Start the HTTP server (default when no subcommand is given).
    Serve,
    /// Probe the running server's `/health` endpoint on loopback.
    ///
    /// Reads `MMCP_BIND` to find the port, connects to
    /// `http://127.0.0.1:<port>/health`, and exits zero when the
    /// response is 2xx. Designed for container orchestrators
    /// (Docker healthcheck, Kubernetes liveness/readiness probes)
    /// that need an in-image tool to hit the health route without
    /// shipping curl or wget in the runtime layer.
    Healthcheck,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command.unwrap_or(Command::Serve) {
        Command::Serve => run_server().await,
        Command::Healthcheck => run_healthcheck().await,
    }
}

async fn run_server() -> Result<()> {
    init_tracing();

    let cfg = config::ServerConfig::from_env();
    tracing::info!(
        address = %cfg.bind,
        database = %cfg.database_url,
        repo_root = %cfg.repo_root.display(),
        "mmcp-server starting"
    );

    let state = state::ServerState::initialize(&cfg).await?;
    let app = app::build_router(state);

    let listener = tokio::net::TcpListener::bind::<SocketAddr>(cfg.bind).await?;
    tracing::info!(address = %cfg.bind, "listening");
    axum::serve(listener, app).await?;
    Ok(())
}

/// GET `/health` on loopback and return zero iff the response is 2xx.
async fn run_healthcheck() -> Result<()> {
    let cfg = config::ServerConfig::from_env();
    let url = format!("http://127.0.0.1:{}/health", cfg.bind.port());
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()
        .context("build healthcheck client")?;
    let response = client
        .get(&url)
        .send()
        .await
        .with_context(|| format!("GET {url}"))?;
    let status = response.status();
    if !status.is_success() {
        bail!("{url} returned {status}");
    }
    Ok(())
}

fn init_tracing() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info,sqlx=warn")),
        )
        .init();
}
