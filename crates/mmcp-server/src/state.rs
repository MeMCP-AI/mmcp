//! Shared server state handed to every handler.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex as StdMutex};

use anyhow::Result;
use mmcp_auth::{MmcpAuthBackend, TokenIssuer, TokenVerifier};
use mmcp_db::{Database, connect};
use mmcp_git::NativeBackend;
use tokio::sync::Mutex as AsyncMutex;
use uuid::Uuid;
use webauthn_rs::prelude::*;

use crate::config::{OAuthProviderConfig, ServerConfig};

/// Everything a request handler needs from the server.
#[derive(Clone)]
pub struct ServerState(pub Arc<ServerStateInner>);

pub struct ServerStateInner {
    pub database: Database,
    pub git: NativeBackend,
    pub repo_root: PathBuf,
    pub token_issuer: TokenIssuer,
    pub token_verifier: TokenVerifier,
    pub auth_backend: MmcpAuthBackend,
    pub webauthn: Arc<Webauthn>,
    pub oauth_providers: HashMap<String, OAuthProviderConfig>,
    pub origin: String,

    /// Shared-secret bearer token that authorizes `git-receive-pack`
    /// (push) requests to every group hosted by this server; see
    /// [`crate::config::ServerConfig::push_token`].
    pub push_token: Option<String>,
    /// Effective minimum accepted account password length, in
    /// bytes; see [`crate::config::ServerConfig::min_password_length`].
    pub min_password_length: usize,
    /// Effective maximum accepted account password length, in
    /// bytes; see [`crate::config::ServerConfig::max_password_length`].
    pub max_password_length: usize,

    /// Per-group async mutex set used to serialize writes (git
    /// `receive-pack`) against the same bare repository. Reads
    /// (`upload-pack`) stay unserialized. Created lazily on first
    /// access for a given group; `std::sync::Mutex` guards only the
    /// HashMap insertion, never a subprocess.
    pub repo_locks: StdMutex<HashMap<Uuid, Arc<AsyncMutex<()>>>>,
}

impl ServerState {
    /// Initialize every subsystem and run migrations.
    pub async fn initialize(cfg: &ServerConfig) -> Result<Self> {
        let database = connect(&cfg.database_url).await?;
        database.migrate().await?;
        let git = NativeBackend::new(&cfg.repo_root)?;
        let token_issuer = TokenIssuer::from_key(&cfg.token_key);
        let token_verifier = TokenVerifier::from_key(&cfg.token_key);
        let auth_backend = MmcpAuthBackend::new(database.connection().clone());

        // WebAuthn relying party derived from the origin.
        let origin_url = Url::parse(&cfg.origin)?;
        let rp_id = origin_url.host_str().unwrap_or("localhost").to_string();
        let rp_origin = origin_url;
        let webauthn = Arc::new(
            WebauthnBuilder::new(&rp_id, &rp_origin)?
                .rp_name("mmcp")
                .build()?,
        );

        let oauth_providers: HashMap<String, OAuthProviderConfig> = cfg
            .oauth_providers
            .iter()
            .map(|p| (p.slug.clone(), p.clone()))
            .collect();

        Ok(Self(Arc::new(ServerStateInner {
            database,
            git,
            repo_root: cfg.repo_root.clone(),
            token_issuer,
            token_verifier,
            auth_backend,
            webauthn,
            oauth_providers,
            origin: cfg.origin.clone(),
            push_token: cfg.push_token.clone(),
            min_password_length: cfg.min_password_length,
            max_password_length: cfg.max_password_length,
            repo_locks: StdMutex::new(HashMap::new()),
        })))
    }

    /// Get (or lazily create) the per-group write lock. The handle
    /// is meant to be held for the duration of a `receive-pack`
    /// invocation so two simultaneous pushes against the same group
    /// repo cannot race and corrupt refs.
    #[must_use]
    pub fn repo_write_lock(&self, group_id: Uuid) -> Arc<AsyncMutex<()>> {
        let mut locks = self.0.repo_locks.lock().expect(
            "repo_locks mutex poisoned: another thread panicked while holding it, leaving the \
             write-serialization map in a possibly inconsistent state that must not be \
             silently continued past",
        );
        // Evict locks nobody currently holds (a strong count of 1
        // means only this map's own entry remains; an active guard
        // clones the `Arc`, so anything actively serializing a push
        // stays above 1) before inserting, so the map does not grow
        // one permanent entry per group ever pushed to for the life
        // of the process.
        locks.retain(|_, lock| Arc::strong_count(lock) > 1);
        locks
            .entry(group_id)
            .or_insert_with(|| Arc::new(AsyncMutex::new(())))
            .clone()
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
