//! mmcp server binary entry point.
//!
//! Hosts the HTTP/SSE MCP surface, the WebUI REST API, the native git
//! smart HTTP endpoint, and the authentication flows. All real wiring
//! is TODO.

use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    tracing::info!("mmcp-server starting (scaffold)");
    // TODO(GGLinnk 2026-04-11): wire axum router, auth, git backend, MCP handlers
    Ok(())
}
