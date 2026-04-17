//! Sync engine wiring shared across every mmcp consumer.
//!
//! `build_engine` constructs a ready-to-use
//! [`SyncEngine`](mmcp_sync::SyncEngine), its
//! [`IndexResolver`] (which maps group UUIDs to the local bare
//! repo via the `GroupIndex` cache), and a fresh
//! [`PendingQueue`](mmcp_sync::PendingQueue). Callers — CLI `mmcp
//! pull` / `mmcp push` / `mmcp sync`, the MCP `sync_pull` /
//! `sync_push` / `sync` tools, and eventually `mmcp-gui`'s sync
//! action handlers — all go through this one helper so the engine
//! configuration never drifts between surfaces.
//!
//! History: ported from `crates/mmcp-client/src/commands/sync.rs`
//! during the FR-020 extraction. The CLI `run` function (clap
//! dispatch + stdout formatting + exit-code mapping) stays in the
//! client crate.

use std::sync::Arc;

use anyhow::{Context, Result};
use mmcp_git::{NativeBackend, RepoHandle};
use mmcp_sync::{GroupHandleResolver, PendingQueue, SyncClient, SyncEngine};
use uuid::Uuid;

use crate::groups::GroupIndex;

/// Build a fresh [`SyncEngine`], [`IndexResolver`], and
/// [`PendingQueue`] pointed at `server_url`.
///
/// Callers supply an already-initialized backend and group index
/// because the MCP server holds them in its state and re-initializing
/// would open a duplicate backend; the CLI builds them once via
/// `MmcpHome::init_backend` and then threads the pair through.
pub fn build_engine(
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

/// Resolver that walks the live [`GroupIndex`] snapshot to answer
/// `resolve` calls from the sync engine.
pub struct IndexResolver {
    /// The live group index; exposed for tests and for callers
    /// that need to poke at the cache after a resolver has been
    /// constructed. Not meaningful outside the sync engine's
    /// trait dispatch.
    pub index: GroupIndex,
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
