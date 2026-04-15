//! Shared server state handed to every handler.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;
use mmcp_auth::{TokenIssuer, TokenVerifier};
use mmcp_db::{Database, connect};
use mmcp_git::NativeBackend;
use uuid::Uuid;

use crate::config::ServerConfig;

/// Everything a request handler needs from the server.
#[derive(Clone)]
pub struct ServerState(pub Arc<ServerStateInner>);

/// Inner state held behind an `Arc` so `Clone` is cheap.
// NOTE: `token_verifier` is wired in by the auth flow once the
// OAuth and passkey routes land. The `allow(dead_code)` is
// intentional and scoped to this struct so the rest of the crate
// still fails on real unused fields.
#[allow(dead_code)]
pub struct ServerStateInner {
    pub database: Database,
    pub git: NativeBackend,
    pub repo_root: PathBuf,
    pub token_issuer: TokenIssuer,
    pub token_verifier: TokenVerifier,
}

impl ServerState {
    /// Initialize every subsystem and run migrations.
    pub async fn initialize(cfg: &ServerConfig) -> Result<Self> {
        let database = connect(&cfg.database_url).await?;
        database.migrate().await?;
        let git = NativeBackend::new(&cfg.repo_root)?;
        let token_issuer = TokenIssuer::from_key(&cfg.token_key);
        let token_verifier = TokenVerifier::from_key(&cfg.token_key);
        Ok(Self(Arc::new(ServerStateInner {
            database,
            git,
            repo_root: cfg.repo_root.clone(),
            token_issuer,
            token_verifier,
        })))
    }

    /// Filesystem path of the bare repository for a group.
    ///
    /// Helper used by the `/sync/*` and `/git/*` routes so they
    /// can compute paths consistently without hard-coding the
    /// `<uuid>.git` convention in each handler. Named with a
    /// `group_` prefix to avoid collision with the `repo_root`
    /// field on `ServerStateInner` that `Deref` would otherwise
    /// shadow.
    #[must_use]
    pub fn group_repo_path(&self, group_id: Uuid) -> PathBuf {
        self.0.repo_root.join(format!("{group_id}.git"))
    }

    /// Borrow the server's repo root directory.
    #[must_use]
    #[allow(dead_code)] // consumed once auth + admin CLI land.
    pub fn repo_root_dir(&self) -> &Path {
        &self.0.repo_root
    }
}

impl std::ops::Deref for ServerState {
    type Target = ServerStateInner;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
