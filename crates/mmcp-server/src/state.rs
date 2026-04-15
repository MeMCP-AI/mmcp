//! Shared server state handed to every handler.

use std::sync::Arc;

use anyhow::Result;
use mmcp_auth::{TokenIssuer, TokenVerifier};
use mmcp_db::{Database, connect};
use mmcp_git::NativeBackend;
use mmcp_session::SessionTracker;

use crate::config::ServerConfig;

/// Everything a request handler needs from the server.
#[derive(Clone)]
pub struct ServerState(pub Arc<ServerStateInner>);

/// Inner state held behind an `Arc` so `Clone` is cheap.
// NOTE: `git`, `sessions`, and `token_verifier` are wired in by
// the auth flow and tool handlers once the read/write surface is
// complete. The `allow(dead_code)` is intentional and scoped to
// this struct so the rest of the crate still fails on real unused
// fields.
#[allow(dead_code)]
pub struct ServerStateInner {
    pub database: Database,
    pub git: NativeBackend,
    pub sessions: SessionTracker,
    pub token_issuer: TokenIssuer,
    pub token_verifier: TokenVerifier,
}

impl ServerState {
    /// Initialize every subsystem and run migrations.
    pub async fn initialize(cfg: &ServerConfig) -> Result<Self> {
        let database = connect(&cfg.database_url).await?;
        database.migrate().await?;
        let git = NativeBackend::new(&cfg.repo_root)?;
        let sessions = SessionTracker::new(database.connection().clone());
        let token_issuer = TokenIssuer::from_key(&cfg.token_key);
        let token_verifier = TokenVerifier::from_key(&cfg.token_key);
        Ok(Self(Arc::new(ServerStateInner {
            database,
            git,
            sessions,
            token_issuer,
            token_verifier,
        })))
    }
}

impl std::ops::Deref for ServerState {
    type Target = ServerStateInner;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
