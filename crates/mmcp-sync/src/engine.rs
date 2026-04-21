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

    /// Pull: fetch into remote-tracking refs, then fast-forward
    /// local `main` to match.
    ///
    /// Git-symmetric two-phase operation:
    ///
    /// 1. Delegate to `self.fetch`, which writes each in-scope
    ///    group's remote head into `refs/remotes/origin/main`
    ///    without touching local `main`.
    /// 2. For each group whose tracking ref advanced, call
    ///    `backend.fast_forward(main, origin/main)` to move local
    ///    `main` forward.
    ///
    /// Divergence (local is not an ancestor of remote) surfaces
    /// today as a silent `FastForwardOutcome::NotFastForward` -
    /// the group still appears in the report so operators know
    /// it was considered, but local `main` is left unchanged.
    /// Step 7 of the sync plan upgrades this to a structured
    /// `pull_diverged` error.
    ///
    /// `filter` and `new_groups` semantics match `fetch`.
    pub async fn pull(
        &self,
        filter: SyncFilter,
        group_handles: &dyn GroupHandleResolver,
        scope_index: &dyn ScopeIndex,
    ) -> Result<PullReport, SyncError> {
        let fetched = self.fetch(filter, group_handles, scope_index).await?;
        let mut updated = Vec::new();
        for fetched_group in fetched.groups {
            // Only attempt the fast-forward when the tracking ref
            // actually moved. `ref_updated=false` means the content
            // plane was skipped (transport failure), so
            // `refs/remotes/origin/main` still points at whatever
            // prior fetch left it at - advancing from that is
            // harmless at best and misleading at worst.
            if fetched_group.ref_updated {
                if let Some(handle) = group_handles.resolve(fetched_group.group_id) {
                    match self
                        .backend
                        .fast_forward(
                            &handle,
                            mmcp_core::conventions::MAIN_BRANCH_REF,
                            mmcp_core::conventions::MAIN_REMOTE_TRACKING_REF,
                        )
                        .await
                    {
                        Ok(_) => {}
                        // Missing tracking ref on first fetch of a
                        // freshly-cloned repo: benign, the local
                        // `main` is already at the target anyway.
                        Err(mmcp_git::GitError::RevNotFound(_)) => {}
                        Err(mmcp_git::GitError::Unsupported(_)) => {}
                        Err(other) => return Err(SyncError::Git(other)),
                    }
                }
            }
            updated.push(crate::client::RemoteGroup {
                group_id: fetched_group.group_id,
                slug: fetched_group.slug,
                head_commit: fetched_group.remote_head,
            });
        }
        Ok(PullReport {
            updated,
            new_groups: fetched.new_groups,
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
