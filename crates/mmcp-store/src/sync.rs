//! Sync engine wiring shared across every mmcp consumer.
//!
//! `build_engine` constructs a ready-to-use [`SyncEngine`](mmcp_sync::SyncEngine) and its [`IndexResolver`],
//! which maps group UUIDs to the local bare repo via the `GroupIndex` cache.
//! Callers, CLI `mmcp pull`/`mmcp push`/`mmcp sync`, the MCP `sync_pull`/`sync_push`/`sync` tools,
//! and `mmcp-gui`'s sync action handlers, all go through this one helper,
//! so the engine configuration never drifts between surfaces.
//!
//! The CLI `run` function (clap dispatch, stdout formatting, exit-code mapping) stays in the client crate.

use std::sync::Arc;

use mmcp_core::manifest::GroupScope;
use mmcp_git::{NativeBackend, RepoHandle};
use mmcp_sync::{GroupHandleResolver, ScopeIndex, SyncClient, SyncEngine};
use uuid::Uuid;

use crate::error::StoreError;
use crate::groups::GroupIndex;

/// Build a fresh [`SyncEngine`] and [`IndexResolver`] pointed at
/// `server_url`.
///
/// Callers supply an already-initialized backend and group index
/// because the MCP server holds them in its state and re-initializing
/// would open a duplicate backend; the CLI builds them once via
/// `MmcpHome::init_backend` and then threads the pair through.
///
/// `token` and `push_token` carry two mutually incompatible
/// credential types, each wired to its own [`SyncClient`] builder
/// method: neither one substitutes for the other, and configuring
/// only one never leaks it into the other's slot.
///
/// - `token` authorizes every request against the `/sync/*` control
///   plane, typically `SyncConfig::resolve_token`'s result: a
///   per-user PASETO session token.
/// - `push_token` authorizes the git content-plane push, typically
///   `SyncConfig::resolve_push_token`'s result: a shared secret
///   compared against the server's `MMCP_PUSH_TOKEN`.
///
/// An absent or empty value in either slot leaves that plane
/// untokened, matching `SyncClient::git_credentials`'s and the
/// `Authorization` header's existing empty-token handling.
pub fn build_engine(
    backend: Arc<NativeBackend>,
    groups: GroupIndex,
    server_url: &str,
    token: Option<&str>,
    push_token: Option<&str>,
) -> Result<(SyncEngine, IndexResolver), StoreError> {
    let mut client = SyncClient::new(server_url.to_owned()).map_err(|source| StoreError::Sync {
        server_url: server_url.to_owned(),
        source,
    })?;
    if let Some(token) = token.filter(|token| !token.is_empty()) {
        client = client.with_bearer(token);
    }
    if let Some(push_token) = push_token.filter(|token| !token.is_empty()) {
        client = client.with_push_credential(push_token);
    }
    let engine = SyncEngine::new(backend, client);
    let resolver = IndexResolver { index: groups };
    Ok((engine, resolver))
}

/// Resolver that walks the live [`GroupIndex`] snapshot to answer
/// `resolve` calls from the sync engine.
pub struct IndexResolver {
    /// The live group index.
    /// Exposed for tests and for callers that need to poke at the cache after a resolver has been constructed.
    /// Not meaningful outside the sync engine's trait dispatch.
    pub index: GroupIndex,
}

impl GroupHandleResolver for IndexResolver {
    fn resolve(&self, group_id: Uuid) -> Option<RepoHandle> {
        // `resolve` runs from a sync context.
        // A short-lived blocking call into the async RwLock is acceptable.
        // The index updates rarely and contention stays minimal in practice.
        // A hot path would require switching the trait method to an async signature.
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

    fn iter_group_ids(&self) -> Vec<Uuid> {
        // `try_list_ids` is the non-blocking snapshot on `GroupIndex`.
        // Same rationale as `scope_of`: the engine calls iter from inside an async runtime, and a `block_on` would panic.
        // An empty result is treated by the engine as "no groups to push", the correct behaviour when the index is mid-rewrite.
        self.index.try_list_ids()
    }
}

impl ScopeIndex for IndexResolver {
    fn scope_of(&self, group_id: Uuid) -> Option<GroupScope> {
        // `try_scope_of` is the non-blocking lookup on `GroupIndex`.
        // This impl stays safe to call from inside an async runtime.
        // Unlike `resolve`, there is no block_on bridge here.
        // The engine's filter dispatch runs on the current worker without a second thread.
        self.index
            .try_scope_of(&mmcp_core::id::GroupId::from_uuid(group_id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::home::MmcpHome;
    use mmcp_core::id::{GroupId, UserId};
    use mmcp_core::manifest::GroupManifest;
    use mmcp_git::GitBackend;
    use tempfile::TempDir;

    #[tokio::test]
    async fn index_resolver_returns_scope_for_known_group() {
        // Seed a tempdir-rooted home with two groups at different
        // scopes so the test pins the lookup path rather than
        // accidentally matching whatever the default scope is.
        let tmp = TempDir::new().expect("tempdir");
        let home = MmcpHome::from_root(tmp.path().join("mmcp-home"));
        let (backend, index) = home.init_backend().await.expect("init backend");

        let shared_id = GroupId::new();
        let mut shared = GroupManifest::new_user_owned(shared_id, "team", UserId::new());
        shared.scope = GroupScope::Shared;
        backend
            .create_group_repo(&shared)
            .await
            .expect("seed shared");

        let project_id = GroupId::new();
        let project = GroupManifest::new_user_owned(project_id, "proj", UserId::new());
        backend
            .create_group_repo(&project)
            .await
            .expect("seed project");
        index.refresh().await.expect("refresh");

        let resolver = IndexResolver { index };
        assert_eq!(
            resolver.scope_of(*shared_id.as_uuid()),
            Some(GroupScope::Shared),
        );
        assert_eq!(
            resolver.scope_of(*project_id.as_uuid()),
            Some(GroupScope::Project),
        );
        assert!(resolver.scope_of(Uuid::now_v7()).is_none());
    }
}
