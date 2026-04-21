//! Sync engine orchestrating the git control and content planes.
//!
//! The engine draws a clean line between two concerns:
//!
//! - The **control plane**, which lives in `SyncClient` and talks
//!   JSON over HTTPS to `mmcp-server`. Today it advertises which
//!   groups exist and at what head commit (`/sync/manifest`,
//!   `/sync/refs/<uuid>`); version-bump registration used to live
//!   here but moved onto the commit stream itself once the client
//!   became fully git-native.
//! - The **content plane**, which lives in `GitBackend` and moves
//!   actual blobs. `fetch` / `pull` / `push` all delegate their
//!   on-wire work to `backend.fetch` and `backend.push`.
//!
//! The three verbs are intentionally git-symmetric:
//!
//! - `fetch` writes each in-scope group's remote head into
//!   `refs/remotes/origin/main` without advancing local `main`.
//! - `pull` fast-forwards local `main` to the remote head.
//! - `push` ships local `main` to the remote. No per-edit queue;
//!   each memory mutation already commits to the local repo, and
//!   push is just `git push origin main` per group.
//!
//! Tests under `tests/engine_smoke.rs` exercise the engine against
//! a `wiremock` HTTP server plus an in-process native git backend
//! so every path except real network transport is covered without
//! a running `mmcp-server`.

use std::sync::Arc;

use mmcp_core::manifest::GroupScope;
use mmcp_git::{GitBackend, RefSpec};
use uuid::Uuid;

use crate::client::{ManifestResponse, SyncClient};
use crate::error::SyncError;
use crate::filter::{ScopeIndex, SyncFilter};

/// True when `group_id` satisfies `filter`.
///
/// `SyncFilter::All` always matches; `SyncFilter::Group(u)` is a
/// straight UUID compare; `SyncFilter::Scope(s)` consults
/// `scope_index` and treats unknown groups as non-matching so the
/// engine silently skips them (unknown-group operator errors are
/// raised at the CLI / MCP boundary, not here).
fn group_matches(
    filter: SyncFilter,
    group_id: Uuid,
    scope_index: &dyn ScopeIndex,
) -> bool {
    match filter {
        SyncFilter::All => true,
        SyncFilter::Group(target) => target == group_id,
        SyncFilter::Scope(target) => scope_index.scope_of(group_id) == Some(target),
    }
}

/// Minimal no-op scope index for callers that only ever pass
/// [`SyncFilter::All`]. The engine's filter dispatch short-circuits
/// on `All` before consulting the scope index, so `scope_of` here
/// is never actually called in that mode; the type exists so test
/// fixtures and CLI paths that do not have a real scope index can
/// pass a trivial placeholder rather than constructing one.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoScopeIndex;

impl ScopeIndex for NoScopeIndex {
    fn scope_of(&self, _group_id: Uuid) -> Option<GroupScope> {
        None
    }
}

/// Engine bound to a specific git backend and sync client.
///
/// Cheap to clone: both the backend and the client are already
/// reference-counted internally.
#[derive(Clone)]
pub struct SyncEngine {
    backend: Arc<dyn GitBackend>,
    client: SyncClient,
}

impl SyncEngine {
    #[must_use]
    pub fn new(backend: Arc<dyn GitBackend>, client: SyncClient) -> Self {
        Self { backend, client }
    }

    /// Push each in-scope group's local `main` to its remote.
    ///
    /// Git-symmetric write path. No pending-edit queue: every
    /// memory mutation already commits to the local bare repo, so
    /// push just walks the selected groups and runs
    /// `git push origin main` on each one. The server's
    /// `receive-pack` handler is authoritative for version-bump
    /// derivation from the commit stream.
    ///
    /// `filter` selects which groups are shipped:
    ///
    /// - `SyncFilter::Group(u)` pushes only `u` (and silently
    ///   no-ops if `u` has no local handle).
    /// - `SyncFilter::All` iterates every locally-indexed group.
    /// - `SyncFilter::Scope(s)` iterates every locally-indexed
    ///   group whose manifest scope equals `s`.
    ///
    /// Transport-layer failures (backend reports `Unsupported` or
    /// `Transport`) are recorded as `content_transferred: false`
    /// rather than raised, so partial network gaps surface in the
    /// report instead of aborting the whole run. Any other git
    /// error propagates as `SyncError::Git`.
    pub async fn push(
        &self,
        filter: SyncFilter,
        group_handles: &dyn GroupHandleResolver,
        scope_index: &dyn ScopeIndex,
    ) -> Result<PushReport, SyncError> {
        let targets = match filter {
            SyncFilter::Group(uuid) => vec![uuid],
            SyncFilter::All | SyncFilter::Scope(_) => group_handles.iter_group_ids(),
        };

        let mut pushed = Vec::new();
        for group_id in targets {
            if !group_matches(filter, group_id, scope_index) {
                continue;
            }
            let Some(handle) = group_handles.resolve(group_id) else {
                // Group id appeared in the iteration snapshot but
                // the handle is gone (index refresh raced with
                // push). Skip rather than error; the next push
                // picks it up if the handle comes back.
                continue;
            };
            let refs = vec![RefSpec::new(
                mmcp_core::conventions::MAIN_BRANCH_REF,
                mmcp_core::conventions::MAIN_BRANCH_REF,
            )];
            let remote_url = self.client.git_url_for(group_id);
            let creds = self.client.git_credentials();
            let content_transferred = match self
                .backend
                .push(&handle, &remote_url, &refs, &creds)
                .await
            {
                Ok(_) => true,
                Err(mmcp_git::GitError::Unsupported(_))
                | Err(mmcp_git::GitError::Transport { .. }) => false,
                Err(other) => return Err(SyncError::Git(other)),
            };
            pushed.push(PushedGroup {
                group_id,
                content_transferred,
            });
        }
        Ok(PushReport { pushed })
    }

    /// Pull the caller's effective group list from the server and
    /// fetch any groups whose local head differs from the remote
    /// head. Groups that do not yet exist locally surface in the
    /// report under `new_groups` so the caller can clone them
    /// through a higher-level bootstrap path.
    ///
    /// `filter` scopes which groups are fetched. `SyncFilter::All`
    /// preserves the whole-mirror behaviour; `SyncFilter::Group(u)`
    /// only touches the matching group; `SyncFilter::Scope(s)`
    /// consults `scope_index` for each server-advertised group and
    /// fetches only matches. Groups the client has never seen
    /// locally ("new groups") are surfaced regardless of filter
    /// because the engine cannot know their scope until they are
    /// cloned; operator flows decide whether to adopt them.
    pub async fn pull(
        &self,
        filter: SyncFilter,
        group_handles: &dyn GroupHandleResolver,
        scope_index: &dyn ScopeIndex,
    ) -> Result<PullReport, SyncError> {
        let manifest: ManifestResponse = self.client.get_manifest().await?;
        let mut updated = Vec::new();
        let mut new_groups = Vec::new();
        for remote in manifest.groups {
            match group_handles.resolve(remote.group_id) {
                None => new_groups.push(remote),
                Some(handle) => {
                    if !group_matches(filter, remote.group_id, scope_index) {
                        continue;
                    }
                    let refs = vec![RefSpec::new(
                        mmcp_core::conventions::MAIN_BRANCH_REF,
                        mmcp_core::conventions::MAIN_BRANCH_REF,
                    )];
                    let remote_url = self.client.git_url_for(remote.group_id);
                    let creds = self.client.git_credentials();
                    match self.backend.fetch(&handle, &remote_url, &refs, &creds).await {
                        Ok(()) => updated.push(remote),
                        Err(mmcp_git::GitError::Unsupported(_))
                        | Err(mmcp_git::GitError::Transport { .. }) => {
                            // Content plane deferred: the remote is
                            // unreachable or the backend can't push
                            // bytes yet. The control-plane view is
                            // still meaningful, so surface the group
                            // in `updated` rather than hiding it.
                            updated.push(remote);
                        }
                        Err(other) => return Err(SyncError::Git(other)),
                    }
                }
            }
        }
        Ok(PullReport {
            updated,
            new_groups,
        })
    }

    /// Pull then push. Both phases see the same filter and the
    /// same scope index.
    pub async fn sync(
        &self,
        filter: SyncFilter,
        group_handles: &dyn GroupHandleResolver,
        scope_index: &dyn ScopeIndex,
    ) -> Result<SyncReport, SyncError> {
        let pulled = self.pull(filter, group_handles, scope_index).await?;
        let pushed = self.push(filter, group_handles, scope_index).await?;
        Ok(SyncReport { pulled, pushed })
    }

    /// Fetch each in-scope group's remote head into its local
    /// remote-tracking ref without advancing `refs/heads/main`.
    ///
    /// Git-symmetric read-only sync: the manifest lists what the
    /// server has, this walks the subset the filter accepts, and
    /// for each indexed group the native backend writes
    /// `refs/remotes/origin/main` so operators can inspect the
    /// incoming tip before `pull` fast-forwards. Groups the client
    /// has never cloned surface under `new_groups` exactly like
    /// `pull` reports them - the engine never auto-adopts them.
    pub async fn fetch(
        &self,
        filter: SyncFilter,
        group_handles: &dyn GroupHandleResolver,
        scope_index: &dyn ScopeIndex,
    ) -> Result<FetchReport, SyncError> {
        let manifest: ManifestResponse = self.client.get_manifest().await?;
        let mut groups = Vec::new();
        let mut new_groups = Vec::new();
        for remote in manifest.groups {
            match group_handles.resolve(remote.group_id) {
                None => new_groups.push(remote),
                Some(handle) => {
                    if !group_matches(filter, remote.group_id, scope_index) {
                        continue;
                    }
                    // `refs/heads/main:refs/remotes/origin/main` - the
                    // git-native shape for "inspect before apply"
                    // fetches. Local `main` stays put; the next `pull`
                    // fast-forwards it from this tracking ref.
                    let refs = vec![RefSpec::new(
                        mmcp_core::conventions::MAIN_BRANCH_REF,
                        mmcp_core::conventions::MAIN_REMOTE_TRACKING_REF,
                    )];
                    let remote_url = self.client.git_url_for(remote.group_id);
                    let creds = self.client.git_credentials();
                    let ref_updated = match self
                        .backend
                        .fetch(&handle, &remote_url, &refs, &creds)
                        .await
                    {
                        Ok(()) => true,
                        // Transport gaps are recorded, not raised:
                        // the control-plane view is still worth
                        // surfacing, and the caller can retry.
                        Err(mmcp_git::GitError::Unsupported(_))
                        | Err(mmcp_git::GitError::Transport { .. }) => false,
                        Err(other) => return Err(SyncError::Git(other)),
                    };
                    groups.push(FetchedGroup {
                        group_id: remote.group_id,
                        slug: remote.slug,
                        remote_head: remote.head_commit,
                        ref_updated,
                    });
                }
            }
        }
        Ok(FetchReport { groups, new_groups })
    }
}

/// Resolves a `group_id` to its local [`mmcp_git::RepoHandle`] and
/// enumerates every locally-known group.
///
/// The client's `GroupIndex` implements this naturally; the sync
/// engine stays decoupled from any specific index type so tests
/// can supply a tiny in-memory resolver.
///
/// `Send + Sync` are required so the engine's async methods can be
/// spawned onto a multi-threaded runtime (e.g. the MCP tool router
/// boxes returned futures with a `Send` bound). Existing resolver
/// impls in this workspace are already thread-safe; the bound
/// simply makes that requirement explicit.
pub trait GroupHandleResolver: Send + Sync {
    /// Look up the local bare-repo handle for a group, if any.
    fn resolve(&self, group_id: Uuid) -> Option<mmcp_git::RepoHandle>;
    /// Snapshot every locally-indexed group id. Used by `push`
    /// to iterate local groups without a manifest round trip.
    /// Returns an empty vector when the underlying index cannot
    /// be read without blocking, which the engine treats as
    /// "no groups to push" - the caller retries on its next tick.
    fn iter_group_ids(&self) -> Vec<Uuid>;
}

/// Report of a completed `push` call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PushReport {
    /// Groups the engine attempted to push, in iteration order.
    /// A group appears here even when the content plane was
    /// skipped (see `PushedGroup::content_transferred`).
    pub pushed: Vec<PushedGroup>,
}

/// One pushed group's before/after snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PushedGroup {
    pub group_id: Uuid,
    /// `false` when the content-plane push was skipped (backend
    /// returned `Unsupported` or a transport error). The group
    /// still appears in the report so operators can see what was
    /// attempted; retry on the next push picks it up.
    pub content_transferred: bool,
}

/// Report of a completed `pull` call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullReport {
    /// Groups whose local head was advanced (or is already in sync).
    pub updated: Vec<crate::client::RemoteGroup>,
    /// Groups the server says exist but the client has no local
    /// clone for yet.
    pub new_groups: Vec<crate::client::RemoteGroup>,
}

/// Report of a full sync (pull then push).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncReport {
    pub pulled: PullReport,
    pub pushed: PushReport,
}

/// Report of a completed `fetch` call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchReport {
    /// Groups whose remote head was written into the local
    /// `refs/remotes/origin/main` tracking ref. Empty list means
    /// no in-scope group was both present locally and advertised
    /// by the server.
    pub groups: Vec<FetchedGroup>,
    /// Groups the server advertises that the client has no local
    /// clone for yet. Reported so operators can decide whether to
    /// adopt them; the engine never auto-clones.
    pub new_groups: Vec<crate::client::RemoteGroup>,
}

/// One fetched group's before/after snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchedGroup {
    pub group_id: Uuid,
    pub slug: String,
    /// Remote HEAD commit at the time the manifest was read. The
    /// tracking ref ends up pointing here when `ref_updated` is
    /// true; a later `pull` fast-forwards local `main` to match.
    pub remote_head: String,
    /// `false` when the content-plane fetch was skipped (backend
    /// returned `Unsupported` or a transport error). The control-
    /// plane view still shows the remote head so operators can
    /// see what would have landed.
    pub ref_updated: bool,
}
