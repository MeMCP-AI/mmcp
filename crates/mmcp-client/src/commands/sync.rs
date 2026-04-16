//! `mmcp sync`, `mmcp pull`, and `mmcp push` implementations.
//!
//! All three subcommands share this entry point: the booleans
//! `pull` and `push` toggle which halves of a full sync run. The
//! body opens a real `SyncClient` pointed at the project's
//! configured server and calls through to `SyncEngine`.
//!
//! The [`build_engine`] helper is `pub(crate)` so the MCP server
//! (`commands/serve.rs`) can build the same engine + resolver pair
//! without duplicating the glue. Both paths share one
//! [`IndexResolver`] implementation.

use std::sync::Arc;

use anyhow::{Context, Result, bail};
use mmcp_git::{NativeBackend, RepoHandle};
use mmcp_sync::{GroupHandleResolver, PendingQueue, SyncClient, SyncEngine, SyncError};
use uuid::Uuid;

use crate::config::{find_project_root, load};
use crate::home::MmcpHome;
use crate::state::GroupIndex;

/// Run the sync engine.
///
/// `pull` and `push` may be toggled independently so that `mmcp
/// pull` and `mmcp push` reuse the same code path.
///
/// Exit codes (returned via `Result`):
///
/// - Ok(()) — sync succeeded (0).
/// - `anyhow::Error` carrying a `SyncError::Conflict` — conflict,
///   caller maps to exit 2.
/// - Any other `anyhow::Error` — generic failure (1).
pub async fn run(pull: bool, push: bool) -> Result<()> {
    let cwd = std::env::current_dir().context("reading current working directory")?;
    let root = find_project_root(&cwd)
        .context("no mmcp project found in current directory or any parent")?;
    let cfg = load(&root)?;

    let Some(sync_cfg) = cfg.sync.as_ref() else {
        bail!(
            "project {} has no [sync] block; cannot sync against a remote",
            cfg.project_uuid
        );
    };

    let mmcp_home = MmcpHome::discover()?;
    let (backend, group_index) = mmcp_home.init_backend().await?;
    let (engine, resolver, queue) = build_engine(backend, group_index, &sync_cfg.server_url)?;

    let report = match (pull, push) {
        (true, true) => {
            let report = engine
                .sync(&queue, &resolver)
                .await
                .map_err(to_anyhow)?;
            tracing::info!(
                server = %sync_cfg.server_url,
                updated = report.pulled.updated.len(),
                new_groups = report.pulled.new_groups.len(),
                drained = report.pushed.drained.len(),
                "sync completed"
            );
            format!(
                "sync against {} completed: pulled {} groups ({} new), pushed {} edits",
                sync_cfg.server_url,
                report.pulled.updated.len(),
                report.pulled.new_groups.len(),
                report.pushed.drained.len()
            )
        }
        (true, false) => {
            let report = engine.pull(&resolver).await.map_err(to_anyhow)?;
            tracing::info!(
                server = %sync_cfg.server_url,
                updated = report.updated.len(),
                new_groups = report.new_groups.len(),
                "pull completed"
            );
            format!(
                "pull from {} completed: {} groups updated, {} new groups",
                sync_cfg.server_url,
                report.updated.len(),
                report.new_groups.len()
            )
        }
        (false, true) => {
            let report = engine.push(&queue, &resolver).await.map_err(to_anyhow)?;
            tracing::info!(
                server = %sync_cfg.server_url,
                drained = report.drained.len(),
                "push completed"
            );
            format!(
                "push to {} completed: {} edits drained",
                sync_cfg.server_url,
                report.drained.len()
            )
        }
        (false, false) => {
            bail!("neither pull nor push requested; nothing to do");
        }
    };

    println!("{report}");
    Ok(())
}

/// Build a fresh [`SyncEngine`], [`IndexResolver`], and
/// [`PendingQueue`] pointed at `server_url`.
///
/// Shared between the CLI (`commands::sync`) and the MCP server
/// (`commands::serve`) so both paths converge on the exact same
/// engine configuration. The caller supplies an already-initialized
/// backend and group index because the MCP server holds them in
/// `ClientState` and re-initializing would open a duplicate backend.
pub(crate) fn build_engine(
    backend: Arc<NativeBackend>,
    groups: GroupIndex,
    server_url: &str,
) -> Result<(SyncEngine, IndexResolver, PendingQueue)> {
    let client = SyncClient::new(server_url.to_owned())
        .with_context(|| format!("configuring sync client for {server_url}"))?;
    let engine = SyncEngine::new(backend, client);
    let resolver = IndexResolver { index: groups };
    let queue = PendingQueue::new();
    Ok((engine, resolver, queue))
}

/// Resolver that walks the live `GroupIndex` snapshot to answer
/// `resolve` calls from the sync engine.
pub(crate) struct IndexResolver {
    pub(crate) index: GroupIndex,
}

impl GroupHandleResolver for IndexResolver {
    fn resolve(&self, group_id: Uuid) -> Option<RepoHandle> {
        // The engine currently calls `resolve` from a sync
        // context. A short-lived blocking call into the async
        // RwLock is acceptable because the index is updated
        // rarely and contention is minimal in practice. If this
        // becomes a hot path we can switch the trait method to
        // an async signature.
        tokio::runtime::Handle::try_current()
            .ok()
            .and_then(|handle| {
                handle.block_on(async {
                    self.index
                        .get(&mmcp_core::id::GroupId::from_uuid(group_id))
                        .await
                })
            })
            .map(|entry| entry.handle)
    }
}

fn to_anyhow(err: SyncError) -> anyhow::Error {
    anyhow::Error::from(err)
}

