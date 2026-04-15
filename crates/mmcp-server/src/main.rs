//! mmcp server binary entry point.

use std::net::SocketAddr;

use anyhow::Result;

mod app;
mod config;
mod routes;
mod state;

#[tokio::main]
async fn main() -> Result<()> {
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

fn init_tracing() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info,sqlx=warn")),
        )
        .init();
}
