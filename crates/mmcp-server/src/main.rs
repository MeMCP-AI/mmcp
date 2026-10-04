//! mmcp server binary entry point.

use std::ffi::OsString;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand};

use mmcp_server::app;
use mmcp_server::config;
use mmcp_server::rate_limit;
use mmcp_server::serve;
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

    /// Directory holding the built web UI, served on every path the API does not own.
    /// It must contain only the built web UI, since every file in it is public.
    /// Overrides MMCP_WEBUI_DIR.
    #[arg(long, env = config::WEBUI_DIR_ENV)]
    webui_dir: Option<OsString>,

    /// Comma-separated origins allowed to call the API from a browser on another origin.
    /// Empty disables CORS.
    /// Overrides MMCP_CORS_ALLOWED_ORIGINS.
    #[arg(long, env = config::CORS_ALLOWED_ORIGINS_ENV)]
    cors_allowed_origins: Option<String>,

    /// Sign-in and registration requests one client may send per minute once its burst is spent.
    /// Highest-precedence tier: beats `MMCP_AUTH_RATE_LIMIT_PER_MINUTE`, the `~/.mmcp/config.toml` `[limits]` tier, and the compiled-in default.
    #[arg(long, env = config::AUTH_RATE_LIMIT_PER_MINUTE_ENV)]
    auth_rate_limit_per_minute: Option<String>,

    /// Sign-in and registration requests one client may send at once before the per-minute rate applies.
    /// Same precedence as `--auth-rate-limit-per-minute`.
    #[arg(long, env = config::AUTH_RATE_LIMIT_BURST_ENV)]
    auth_rate_limit_burst: Option<String>,

    /// Leading bits of an IPv6 address that identify one client for the sign-in and registration limit.
    /// Every address that starts with the same bits counts as one client.
    /// Same precedence as `--auth-rate-limit-per-minute`.
    #[arg(long, env = config::AUTH_RATE_LIMIT_IPV6_CLIENT_PREFIX_LENGTH_ENV)]
    auth_rate_limit_ipv6_client_prefix_length: Option<String>,

    /// Leading bits of an IPv6 address that identify one network for the second sign-in and registration limit.
    /// It counts every client of one network together, so one network cannot get around the limit by using many addresses.
    /// Must be shorter than the client prefix length.
    /// Set it to 0 to switch the network limit off.
    /// Same precedence as `--auth-rate-limit-per-minute`.
    #[arg(long, env = config::AUTH_RATE_LIMIT_IPV6_NETWORK_PREFIX_LENGTH_ENV)]
    auth_rate_limit_ipv6_network_prefix_length: Option<String>,

    /// Sign-in and registration requests one IPv6 network may send per minute once its burst is spent.
    /// Unset means 10 times the client value.
    /// Same precedence as `--auth-rate-limit-per-minute`.
    #[arg(long, env = config::AUTH_RATE_LIMIT_IPV6_NETWORK_PER_MINUTE_ENV)]
    auth_rate_limit_ipv6_network_per_minute: Option<String>,

    /// Sign-in and registration requests one IPv6 network may send at once before the per-minute rate applies.
    /// Unset means 10 times the client value.
    /// Same precedence as `--auth-rate-limit-per-minute`.
    #[arg(long, env = config::AUTH_RATE_LIMIT_IPV6_NETWORK_BURST_ENV)]
    auth_rate_limit_ipv6_network_burst: Option<String>,

    /// Comma-separated IP addresses or CIDR ranges of the reverse proxies in front of this server.
    /// A request coming from one of them is attributed to the client address its X-Forwarded-For header gives.
    /// List only proxies that clients cannot bypass, and leave it unset when clients connect to the server directly.
    /// Beats MMCP_TRUSTED_PROXIES.
    #[arg(long, env = config::TRUSTED_PROXIES_ENV)]
    trusted_proxies: Option<String>,
}

impl From<ServeArgs> for config::ServerConfigOverrides {
    fn from(args: ServeArgs) -> Self {
        Self {
            min_password_length: args.min_password_length,
            max_password_length: args.max_password_length,
            max_handle_length: args.max_handle_length,
            webui_dir: args.webui_dir.map(PathBuf::from),
            cors_allowed_origins: args.cors_allowed_origins,
            auth_rate_limit: config::AuthRateLimitOverrides {
                per_minute: args.auth_rate_limit_per_minute,
                burst: args.auth_rate_limit_burst,
                ipv6_client_prefix_length: args.auth_rate_limit_ipv6_client_prefix_length,
                ipv6_network_prefix_length: args.auth_rate_limit_ipv6_network_prefix_length,
                ipv6_network_per_minute: args.auth_rate_limit_ipv6_network_per_minute,
                ipv6_network_burst: args.auth_rate_limit_ipv6_network_burst,
            },
            trusted_proxies: args.trusted_proxies,
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
    // Boxed: the serve arguments are far larger than the other variants.
    Serve(Box<ServeArgs>),
    /// Probe the running server's `/health` endpoint on loopback.
    ///
    /// Reads `MMCP_BIND` to find the port, connects to
    /// `http://127.0.0.1:<port>/health`, and exits zero when the
    /// response is 2xx. Designed for container orchestrators
    /// (Docker healthcheck, Kubernetes liveness/readiness probes)
    /// that need an in-image tool to hit the health route without
    /// shipping curl or wget in the runtime layer.
    Healthcheck,
    /// Print a random 256-bit secret as 64 hexadecimal characters.
    /// Suitable for MMCP_TOKEN_KEY_HEX and POSTGRES_PASSWORD.
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
        Some(Command::Serve(args)) => run_server((*args).into()).await,
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
    let rate_limiter = state.auth_rate_limiter.clone();
    tracing::info!(
        limits = %cfg.auth_rate_limit,
        trusted_proxies = %cfg.trusted_proxies,
        "auth rate limit active"
    );
    let app = app::build_router(state);

    let listener = tokio::net::TcpListener::bind::<SocketAddr>(cfg.bind).await?;
    tracing::info!(address = %cfg.bind, "listening");
    // Held until the server stops, when dropping it aborts the sweep.
    let _session_sweeper = session_store::spawn_expired_session_sweeper(session_store);
    let _rate_limit_sweeper = rate_limit::spawn_auth_rate_limit_sweeper(rate_limiter);
    serve::serve_router(listener, app).await?;
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
    fn serve_auth_rate_limit_and_trusted_proxies_are_bound_to_their_environment_variables() {
        assert_eq!(
            serve_arg_env("auth_rate_limit_per_minute").as_deref(),
            Some(config::AUTH_RATE_LIMIT_PER_MINUTE_ENV)
        );
        assert_eq!(
            serve_arg_env("auth_rate_limit_burst").as_deref(),
            Some(config::AUTH_RATE_LIMIT_BURST_ENV)
        );
        assert_eq!(
            serve_arg_env("trusted_proxies").as_deref(),
            Some(config::TRUSTED_PROXIES_ENV)
        );
    }

    #[test]
    fn serve_ipv6_rate_limit_settings_are_bound_to_their_environment_variables() {
        assert_eq!(
            serve_arg_env("auth_rate_limit_ipv6_client_prefix_length").as_deref(),
            Some(config::AUTH_RATE_LIMIT_IPV6_CLIENT_PREFIX_LENGTH_ENV)
        );
        assert_eq!(
            serve_arg_env("auth_rate_limit_ipv6_network_prefix_length").as_deref(),
            Some(config::AUTH_RATE_LIMIT_IPV6_NETWORK_PREFIX_LENGTH_ENV)
        );
        assert_eq!(
            serve_arg_env("auth_rate_limit_ipv6_network_per_minute").as_deref(),
            Some(config::AUTH_RATE_LIMIT_IPV6_NETWORK_PER_MINUTE_ENV)
        );
        assert_eq!(
            serve_arg_env("auth_rate_limit_ipv6_network_burst").as_deref(),
            Some(config::AUTH_RATE_LIMIT_IPV6_NETWORK_BURST_ENV)
        );
    }

    #[test]
    fn serve_rate_limit_flags_reach_the_config_override_tier() {
        let cli = Cli::try_parse_from([
            BINARY_NAME,
            "serve",
            "--auth-rate-limit-per-minute",
            "30",
            "--auth-rate-limit-burst",
            "5",
            "--auth-rate-limit-ipv6-client-prefix-length",
            "56",
            "--auth-rate-limit-ipv6-network-prefix-length",
            "40",
            "--auth-rate-limit-ipv6-network-per-minute",
            "300",
            "--auth-rate-limit-ipv6-network-burst",
            "50",
            "--trusted-proxies",
            "10.0.0.0/8,192.0.2.1",
        ])
        .unwrap();
        let Some(Command::Serve(args)) = cli.command else {
            panic!("expected the serve subcommand");
        };
        let overrides = config::ServerConfigOverrides::from(*args);
        let rate_limit = &overrides.auth_rate_limit;
        assert_eq!(rate_limit.per_minute.as_deref(), Some("30"));
        assert_eq!(rate_limit.burst.as_deref(), Some("5"));
        assert_eq!(rate_limit.ipv6_client_prefix_length.as_deref(), Some("56"));
        assert_eq!(rate_limit.ipv6_network_prefix_length.as_deref(), Some("40"));
        assert_eq!(rate_limit.ipv6_network_per_minute.as_deref(), Some("300"));
        assert_eq!(rate_limit.ipv6_network_burst.as_deref(), Some("50"));
        assert_eq!(
            overrides.trusted_proxies.as_deref(),
            Some("10.0.0.0/8,192.0.2.1")
        );
    }

    #[test]
    fn blank_rate_limit_and_trusted_proxy_flags_are_accepted_rather_than_rejected_at_startup() {
        let cli = Cli::try_parse_from([
            BINARY_NAME,
            "serve",
            "--auth-rate-limit-per-minute",
            "",
            "--auth-rate-limit-burst",
            "",
            "--auth-rate-limit-ipv6-client-prefix-length",
            "",
            "--auth-rate-limit-ipv6-network-prefix-length",
            "",
            "--auth-rate-limit-ipv6-network-per-minute",
            "",
            "--auth-rate-limit-ipv6-network-burst",
            "",
            "--trusted-proxies",
            "",
        ])
        .unwrap();
        let Some(Command::Serve(args)) = cli.command else {
            panic!("expected the serve subcommand");
        };
        let overrides = config::ServerConfigOverrides::from(*args);
        let rate_limit = &overrides.auth_rate_limit;
        assert_eq!(rate_limit.per_minute.as_deref(), Some(""));
        assert_eq!(rate_limit.burst.as_deref(), Some(""));
        assert_eq!(rate_limit.ipv6_client_prefix_length.as_deref(), Some(""));
        assert_eq!(rate_limit.ipv6_network_prefix_length.as_deref(), Some(""));
        assert_eq!(rate_limit.ipv6_network_per_minute.as_deref(), Some(""));
        assert_eq!(rate_limit.ipv6_network_burst.as_deref(), Some(""));
        assert_eq!(overrides.trusted_proxies.as_deref(), Some(""));
    }

    #[test]
    fn web_ui_dir_help_warns_that_every_file_in_the_directory_is_public() {
        let command = Cli::command();
        let serve = command.find_subcommand("serve").expect("serve subcommand");
        let arg = serve
            .get_arguments()
            .find(|arg| arg.get_id() == "webui_dir")
            .expect("serve argument");
        let help = arg.get_help().expect("webui_dir has help").to_string();
        assert!(help.contains("every file in it is public"), "{help}");
    }

    #[test]
    fn the_no_subcommand_parser_exposes_the_same_serve_arguments() {
        let command = ServeOnlyCli::command();
        for arg_id in [
            "webui_dir",
            "cors_allowed_origins",
            "auth_rate_limit_per_minute",
            "auth_rate_limit_burst",
            "auth_rate_limit_ipv6_client_prefix_length",
            "auth_rate_limit_ipv6_network_prefix_length",
            "auth_rate_limit_ipv6_network_per_minute",
            "auth_rate_limit_ipv6_network_burst",
            "trusted_proxies",
        ] {
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
        let overrides = config::ServerConfigOverrides::from(*args);
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
        let overrides = config::ServerConfigOverrides::from(*args);
        assert_eq!(overrides.webui_dir, Some(PathBuf::new()));
        assert_eq!(overrides.cors_allowed_origins.as_deref(), Some(""));
    }
}
