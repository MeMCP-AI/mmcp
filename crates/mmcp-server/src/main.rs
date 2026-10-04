//! mmcp server binary entry point.

use std::ffi::OsString;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand};

use mmcp_server::app;
use mmcp_server::config;
use mmcp_server::session_store;
use mmcp_server::state;

const BINARY_NAME: &str = "mmcp-server";

#[derive(Parser)]
#[command(
    name = BINARY_NAME,
    version,
    about = "mmcp server daemon: MCP transport, git smart-HTTP, auth, WebUI API"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

/// Arguments of the `serve` subcommand.
#[derive(Args)]
struct ServeArgs {
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

    #[arg(long, env = config::WEBUI_DIR_ENV)]
    webui_dir: Option<OsString>,

    #[arg(long, env = config::CORS_ALLOWED_ORIGINS_ENV)]
    cors_allowed_origins: Option<String>,
}

impl From<ServeArgs> for config::ServerConfigOverrides {
    fn from(args: ServeArgs) -> Self {
        Self {
            min_password_length: args.min_password_length,
            max_password_length: args.max_password_length,
            max_handle_length: args.max_handle_length,
            webui_dir: args.webui_dir.map(PathBuf::from),
            cors_allowed_origins: args.cors_allowed_origins,
        }
    }
}

/// The `serve` arguments alone, parsed when no subcommand is given.
#[derive(Parser)]
#[command(name = BINARY_NAME)]
struct ServeOnlyCli {
    #[command(flatten)]
    serve: ServeArgs,
}

#[derive(Subcommand)]
enum Command {
    /// Start the HTTP server (default when no subcommand is given).
    Serve(ServeArgs),
    /// Probe the running server's `/health` endpoint on loopback.
    ///
    /// Reads `MMCP_BIND` to find the port, connects to
    /// `http://127.0.0.1:<port>/health`, and exits zero when the
    /// response is 2xx. Designed for container orchestrators
    /// (Docker healthcheck, Kubernetes liveness/readiness probes)
    /// that need an in-image tool to hit the health route without
    /// shipping curl or wget in the runtime layer.
    Healthcheck,
    GenerateSecret,
}

/// `serve` arguments for a caller that passes no subcommand.
/// They are parsed from an empty `serve` invocation, so clap still resolves their environment variables.
fn default_serve_args() -> ServeArgs {
    ServeOnlyCli::parse_from([BINARY_NAME]).serve
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Some(Command::Serve(args)) => run_server(args.into()).await,
        None => run_server(default_serve_args().into()).await,
        Some(Command::Healthcheck) => run_healthcheck().await,
        Some(Command::GenerateSecret) => {
            println!("{}", config::generate_secret_hex()?);
            Ok(())
        }
    }
}

async fn run_server(overrides: config::ServerConfigOverrides) -> Result<()> {
    init_tracing();

    let cfg = config::ServerConfig::from_env_with_overrides(overrides)?;
    tracing::info!(
        address = %cfg.bind,
        database = %cfg.redacted_database_url(),
        repo_root = %cfg.repo_root.display(),
        "mmcp-server starting"
    );

    let state = state::ServerState::initialize(&cfg).await?;
    let session_store = state.session_store.clone();
    let app = app::build_router(state);

    let listener = tokio::net::TcpListener::bind::<SocketAddr>(cfg.bind).await?;
    tracing::info!(address = %cfg.bind, "listening");
    let sweeper = session_store::spawn_expired_session_sweeper(session_store);
    let served = axum::serve(listener, app).await;
    sweeper.abort();
    served?;
    Ok(())
}

/// GET `/health` on loopback and return zero iff the response is 2xx.
async fn run_healthcheck() -> Result<()> {
    let cfg = config::ServerConfig::from_env()?;
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

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use clap::CommandFactory;

    use super::*;

    /// Environment variable bound to the argument `arg_id` of the `serve` subcommand.
    fn serve_arg_env(arg_id: &str) -> Option<String> {
        let command = Cli::command();
        let serve = command.find_subcommand("serve").expect("serve subcommand");
        let arg = serve
            .get_arguments()
            .find(|arg| arg.get_id() == arg_id)
            .expect("serve argument");
        arg.get_env()
            .map(|name| name.to_string_lossy().into_owned())
    }

    #[test]
    fn serve_web_ui_dir_and_cors_origins_are_bound_to_their_environment_variables() {
        assert_eq!(
            serve_arg_env("webui_dir").as_deref(),
            Some(config::WEBUI_DIR_ENV)
        );
        assert_eq!(
            serve_arg_env("cors_allowed_origins").as_deref(),
            Some(config::CORS_ALLOWED_ORIGINS_ENV)
        );
    }

    #[test]
    fn the_no_subcommand_parser_exposes_the_same_serve_arguments() {
        let command = ServeOnlyCli::command();
        for arg_id in ["webui_dir", "cors_allowed_origins"] {
            let arg = command
                .get_arguments()
                .find(|arg| arg.get_id() == arg_id)
                .expect("serve argument");
            assert!(arg.get_env().is_some(), "{arg_id}");
        }
    }

    #[test]
    fn serve_flags_reach_the_config_override_tier() {
        let cli = Cli::try_parse_from([
            BINARY_NAME,
            "serve",
            "--webui-dir",
            "/usr/share/mmcp/webui",
            "--cors-allowed-origins",
            "http://127.0.0.1:3000",
        ])
        .unwrap();
        let Some(Command::Serve(args)) = cli.command else {
            panic!("expected the serve subcommand");
        };
        let overrides = config::ServerConfigOverrides::from(args);
        assert_eq!(
            overrides.webui_dir,
            Some(PathBuf::from("/usr/share/mmcp/webui"))
        );
        assert_eq!(
            overrides.cors_allowed_origins.as_deref(),
            Some("http://127.0.0.1:3000")
        );
    }

    #[test]
    fn blank_flag_values_are_accepted_rather_than_rejected_at_startup() {
        let cli = Cli::try_parse_from([
            BINARY_NAME,
            "serve",
            "--webui-dir",
            "",
            "--cors-allowed-origins",
            "",
        ])
        .unwrap();
        let Some(Command::Serve(args)) = cli.command else {
            panic!("expected the serve subcommand");
        };
        let overrides = config::ServerConfigOverrides::from(args);
        assert_eq!(overrides.webui_dir, Some(PathBuf::new()));
        assert_eq!(overrides.cors_allowed_origins.as_deref(), Some(""));
    }
}
