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
    Serve {
        /// Override the minimum accepted account password length, in
        /// bytes. Highest-precedence tier: beats
        /// `MMCP_MIN_PASSWORD_LENGTH`, the `~/.mmcp/config.toml`
        /// `[limits]` tier, and the compiled-in default.
        #[arg(long)]
        min_password_length: Option<usize>,

        /// Override the maximum accepted account password length, in
        /// bytes. Same precedence as `--min-password-length`.
        #[arg(long)]
        max_password_length: Option<usize>,

        /// Override the maximum accepted account handle length, in bytes.
        /// Same precedence as `--min-password-length`.
        #[arg(long)]
        max_handle_length: Option<usize>,
    },
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

/// Default `Command::Serve` used when the caller passes no
/// subcommand at all: no override tier, matching
/// `ServerConfig::from_env`'s own no-CLI-override default.
fn default_serve_command() -> Command {
    Command::Serve {
        min_password_length: None,
        max_password_length: None,
        max_handle_length: None,
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command.unwrap_or_else(default_serve_command) {
        Command::Serve {
            min_password_length,
            max_password_length,
            max_handle_length,
        } => {
            run_server(config::ServerConfigOverrides {
                min_password_length,
                max_password_length,
                max_handle_length,
            })
            .await
        }
        Command::Healthcheck => run_healthcheck().await,
    }
}

async fn run_server(overrides: config::ServerConfigOverrides) -> Result<()> {
    init_tracing();

    let cfg = config::ServerConfig::from_env_with_overrides(overrides);
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
