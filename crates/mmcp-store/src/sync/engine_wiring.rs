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

use mmcp_core::config::{Remote, RemoteAuth, SyncConfig};
use mmcp_core::manifest::GroupScope;
use mmcp_git::{NativeBackend, RepoHandle};
use mmcp_sync::{
    BoundRemote, GroupHandleResolver, RemoteTransport, ScopeIndex, SyncClient, SyncEngine,
};
use uuid::Uuid;

use super::remotes::{EffectiveRemotes, ResolvedRemote};
use crate::error::StoreError;
use crate::groups::GroupIndex;

/// Build a fresh [`SyncEngine`] and [`IndexResolver`] wired from
/// `effective`, one [`BoundRemote`] per resolved remote.
///
/// Reads real process environment variables for every remote's
/// credentials, per the derivation rule documented on
/// [`SyncConfig::token_env_name`] / [`SyncConfig::push_token_env_name`].
/// See [`build_engine_with_env`] for the test-injectable form.
///
/// # Errors
/// [`StoreError::Sync`] if an `mmcp-server` remote's `SyncClient`
/// fails to construct, or [`StoreError::DirectGitGroupNotFound`] if a
/// `direct-git` remote's resolved group reference does not resolve
/// against the local mirror.
pub async fn build_engine(
    backend: Arc<NativeBackend>,
    groups: GroupIndex,
    effective: &EffectiveRemotes,
) -> Result<(SyncEngine, IndexResolver), StoreError> {
    build_engine_with_env(backend, groups, effective, |key| std::env::var(key).ok()).await
}

/// Same as [`build_engine`], with the environment lookup injected so
/// tests never race on `std::env` under cargo's parallel runner
/// (mirrors [`SyncConfig::resolve_token_with`]'s pattern). `pub`
/// rather than `pub(crate)`: `mmcp-store`'s own external integration
/// tests need this seam from outside the crate.
pub async fn build_engine_with_env(
    backend: Arc<NativeBackend>,
    groups: GroupIndex,
    effective: &EffectiveRemotes,
    get_env: impl Fn(&str) -> Option<String>,
) -> Result<(SyncEngine, IndexResolver), StoreError> {
    let mut bound = Vec::with_capacity(effective.remotes.len());
    for (index, resolved) in effective.remotes.iter().enumerate() {
        let transport = build_transport(resolved, &groups, &get_env).await?;
        bound.push(BoundRemote {
            name: resolved.name().to_string(),
            default: effective.default_index == Some(index),
            include_in_push_all: resolved.remote.include_in_push_all(),
            transport,
        });
    }
    let engine = SyncEngine::new(backend, bound);
    let resolver = IndexResolver { index: groups };
    Ok((engine, resolver))
}

/// Build one resolved remote's [`RemoteTransport`].
async fn build_transport(
    resolved: &ResolvedRemote,
    groups: &GroupIndex,
    get_env: &impl Fn(&str) -> Option<String>,
) -> Result<RemoteTransport, StoreError> {
    match &resolved.remote {
        Remote::MmcpServer { url, .. } => {
            let (token, push_token) = resolve_credentials(resolved, get_env);
            let mut client = SyncClient::new(url.clone()).map_err(|source| StoreError::Sync {
                server_url: url.clone(),
                source,
            })?;
            if let Some(token) = token.filter(|t| !t.is_empty()) {
                client = client.with_bearer(token);
            }
            if let Some(push_token) = push_token.filter(|t| !t.is_empty()) {
                client = client.with_push_credential(push_token);
            }
            Ok(RemoteTransport::MmcpServer(client))
        }
        Remote::DirectGit { url, auth, .. } => {
            // The resolver populates `direct_git_group` for every
            // `DirectGit` entry it produces; an absent value here
            // would mean a `ResolvedRemote` was hand-built outside
            // that contract (a test, or a future caller). Rather
            // than panic on that impossible-in-practice case, fall
            // through to the same not-found path an empty group
            // reference would take, and let the caller see a real,
            // structured error instead of a crash.
            let group_ref = resolved.direct_git_group.as_deref().unwrap_or_default();
            let group_id = resolve_group_id(resolved.name(), group_ref, groups).await?;
            let creds = match auth {
                RemoteAuth::None | RemoteAuth::SshAgent => mmcp_git::Credentials::None,
                // A direct-git remote has one credential slot, used
                // for both fetch and push; the control-plane `token`
                // side (not `push_token`, an mmcp-server-specific
                // control/content split with no direct-git
                // equivalent) is the natural analogue of a plain git
                // host personal-access token.
                RemoteAuth::Bearer => {
                    let (token, _push_token) = resolve_credentials(resolved, get_env);
                    match token.filter(|t| !t.is_empty()) {
                        Some(token) => mmcp_git::Credentials::bearer(token),
                        None => mmcp_git::Credentials::None,
                    }
                }
            };
            Ok(RemoteTransport::DirectGit {
                url: url.clone(),
                creds,
                group_id,
            })
        }
    }
}

/// Resolve a `direct-git` remote's declared/defaulted group
/// reference (UUID or slug) against the local mirror.
async fn resolve_group_id(
    remote_name: &str,
    group_ref: &str,
    groups: &GroupIndex,
) -> Result<Uuid, StoreError> {
    let entry = crate::memory::resolve_group(groups, group_ref)
        .await
        .map_err(|source| StoreError::DirectGitGroupNotFound {
            remote_name: remote_name.to_string(),
            group_ref: group_ref.to_string(),
            source,
        })?;
    Ok(*entry.manifest.group_id.as_uuid())
}

/// Resolve `resolved`'s control-plane bearer and content-plane push
/// credentials from the environment.
///
/// A legacy `server_url`-shorthand entry keeps reading the existing
/// un-suffixed `SYNC_TOKEN_ENV` / `SYNC_PUSH_TOKEN_ENV` constants,
/// unchanged from the single-remote era. Every other remote (named
/// `mmcp-server` or `direct-git`) resolves via the name-derived
/// `SyncConfig::token_env_name` / `push_token_env_name` env vars.
fn resolve_credentials(
    resolved: &ResolvedRemote,
    get_env: &impl Fn(&str) -> Option<String>,
) -> (Option<String>, Option<String>) {
    if resolved.is_legacy_shorthand() {
        (
            get_env(mmcp_core::config::SYNC_TOKEN_ENV),
            get_env(mmcp_core::config::SYNC_PUSH_TOKEN_ENV),
        )
    } else {
        (
            get_env(&SyncConfig::token_env_name(resolved.name())),
            get_env(&SyncConfig::push_token_env_name(resolved.name())),
        )
    }
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
    #![allow(clippy::unwrap_used, clippy::expect_used)]
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

    /// `build_engine_with_env` maps one `EffectiveRemotes` entry per
    /// `BoundRemote`, preserving `default`/`include_in_push_all`, and
    /// resolves the legacy shorthand's un-suffixed env vars.
    #[tokio::test]
    async fn build_engine_wires_one_bound_remote_per_effective_entry() {
        let tmp = TempDir::new().expect("tempdir");
        let home = MmcpHome::from_root(tmp.path().join("mmcp-home"));
        let (backend, groups) = home.init_backend().await.expect("init backend");

        let effective = EffectiveRemotes {
            remotes: vec![ResolvedRemote {
                remote: Remote::MmcpServer {
                    name: "user-legacy".to_string(),
                    url: "https://mmcp.example.com".to_string(),
                    default: false,
                    include_in_push_all: true,
                },
                level: super::super::remotes::RemoteLevel::User,
                direct_git_group: None,
            }],
            default_index: Some(0),
        };

        let (engine, _resolver) =
            build_engine_with_env(backend, groups, &effective, |key| match key {
                "MMCP_SYNC_TOKEN" => Some("legacy-bearer".to_string()),
                _ => None,
            })
            .await
            .expect("build engine");

        assert_eq!(engine.remotes().len(), 1);
        assert!(engine.remotes()[0].default);
        assert_eq!(engine.remotes()[0].name, "user-legacy");
    }

    /// A named (non-legacy) remote reads the name-derived env vars,
    /// never the un-suffixed legacy ones.
    #[tokio::test]
    async fn build_engine_reads_derived_env_vars_for_a_named_remote() {
        let tmp = TempDir::new().expect("tempdir");
        let home = MmcpHome::from_root(tmp.path().join("mmcp-home"));
        let (backend, groups) = home.init_backend().await.expect("init backend");

        let effective = EffectiveRemotes {
            remotes: vec![ResolvedRemote {
                remote: Remote::MmcpServer {
                    name: "prod-eu".to_string(),
                    url: "https://mmcp.example.com".to_string(),
                    default: true,
                    include_in_push_all: true,
                },
                level: super::super::remotes::RemoteLevel::Project,
                direct_git_group: None,
            }],
            default_index: Some(0),
        };

        let (engine, _resolver) = build_engine_with_env(backend, groups, &effective, |key| {
            match key {
                "MMCP_SYNC_PUSH_TOKEN_PROD_EU" => Some("derived-push".to_string()),
                // The legacy env var must never leak into a named
                // remote's credential; returning a value for it here
                // would let this test pass for the wrong reason.
                "MMCP_SYNC_PUSH_TOKEN" => Some("legacy-push".to_string()),
                _ => None,
            }
        })
        .await
        .expect("build engine");

        assert_eq!(
            engine.remotes()[0].git_credentials(),
            mmcp_git::Credentials::bearer("derived-push")
        );
    }

    /// A `direct-git` remote whose resolved group reference does not
    /// exist locally surfaces `StoreError::DirectGitGroupNotFound`
    /// rather than panicking or silently building a broken engine.
    #[tokio::test]
    async fn build_engine_reports_an_unresolvable_direct_git_group() {
        let tmp = TempDir::new().expect("tempdir");
        let home = MmcpHome::from_root(tmp.path().join("mmcp-home"));
        let (backend, groups) = home.init_backend().await.expect("init backend");

        let effective = EffectiveRemotes {
            remotes: vec![ResolvedRemote {
                remote: Remote::DirectGit {
                    name: "mirror".to_string(),
                    url: "ssh://git@example.com/mirror.git".to_string(),
                    auth: RemoteAuth::None,
                    group: Some("ghost-group".to_string()),
                    default: true,
                    include_in_push_all: true,
                },
                level: super::super::remotes::RemoteLevel::Project,
                direct_git_group: Some("ghost-group".to_string()),
            }],
            default_index: Some(0),
        };

        let result = build_engine_with_env(backend, groups, &effective, |_| None).await;
        let err = match result {
            Err(err) => err,
            Ok(_) => panic!("unresolvable group must error"),
        };
        assert!(matches!(
            err,
            StoreError::DirectGitGroupNotFound { remote_name, group_ref, .. }
                if remote_name == "mirror" && group_ref == "ghost-group"
        ));
    }
}
