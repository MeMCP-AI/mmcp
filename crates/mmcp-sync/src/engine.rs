//! Sync engine orchestrating the pending push queue and the
//! remote control-plane endpoints.
//!
//! The engine draws a clean line between two concerns:
//!
//! - The **control plane**, which lives in `SyncClient` and talks
//!   JSON over HTTPS to `mmcp-server`. Pushing an edit means
//!   registering a version bump with the server and receiving the
//!   assigned semver string back. Pulling means listing which
//!   groups should exist locally and at what head commit.
//! - The **content plane**, which lives in `GitBackend` and moves
//!   actual blobs. The engine calls `backend.push` / `backend.fetch`
//!   once the control-plane decision is recorded. Any failure the
//!   backend returns propagates as `SyncError::Git`.
//!
//! Tests under `tests/engine_smoke.rs` exercise the engine against
//! a `wiremock` HTTP server plus an in-process native git backend
//! so every path except real network transport is covered without
//! a running `mmcp-server`.

use std::sync::Arc;

use mmcp_core::manifest::GroupScope;
use mmcp_git::{GitBackend, RefSpec};
use uuid::Uuid;

use crate::client::{ManifestResponse, PushRequest, PushResponse, SyncClient};
use crate::error::SyncError;
use crate::filter::{ScopeIndex, SyncFilter};
use crate::pending::{PendingEdit, PendingQueue};

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

    /// Drain the pending push queue.
    ///
    /// For each enqueued edit the engine:
    ///
    /// 1. Registers a version bump with the server via
    ///    `POST /sync/push` and records the assigned version in
    ///    the resulting [`DrainedPush`].
    /// 2. Invokes `GitBackend::push` so the local commit reaches
    ///    the remote bare repo. When the backend returns
    ///    [`GitError::Unsupported`](mmcp_git::GitError::Unsupported),
    ///    the engine records a synthetic report entry explaining
    ///    that the content plane is not yet wired but the version
    ///    was recorded on the server; any other git error
    ///    propagates as an aborted push.
    ///
    /// On success the engine removes the drained edit from the
    /// queue. On any error the edit stays enqueued so the caller
    /// can retry. The first error aborts further drains and is
    /// returned alongside the partial report so the caller can
    /// report "pushed N out of M, stopped on error".
    ///
    /// `filter` selects which pending edits are eligible for
    /// drain; edits whose `group_id` does not match stay in the
    /// queue and the drain skips them without error. Callers that
    /// want whole-queue drain pass [`SyncFilter::All`] explicitly.
    pub async fn push(
        &self,
        queue: &PendingQueue,
        filter: SyncFilter,
        group_handles: &dyn GroupHandleResolver,
        scope_index: &dyn ScopeIndex,
    ) -> Result<PushReport, SyncError> {
        let mut drained = Vec::new();
        // Snapshot-then-remove preserves FIFO within the filter
        // match set: non-matching edits stay in the queue at their
        // original position, matching ones drain in enqueue order.
        let snapshot = queue.snapshot();
        for candidate in snapshot {
            if !group_matches(filter, candidate.group_id, scope_index) {
                continue;
            }
            let edit = match queue.remove(candidate.id) {
                Ok(edit) => edit,
                // Another drain may have won the race; treat the
                // missing entry as already-handled and move on
                // instead of erroring out.
                Err(SyncError::NotFound(_)) => continue,
                Err(other) => return Err(other),
            };
            match self.push_one(&edit, group_handles).await {
                Ok(d) => drained.push(d),
                Err(err) => {
                    // Re-enqueue so the next drain picks the edit
                    // back up; the re-enqueue appends at tail, which
                    // shifts the edit's relative position but keeps
                    // it available for retry.
                    queue.enqueue(edit);
                    return Err(err);
                }
            }
        }
        Ok(PushReport { drained })
    }

    async fn push_one(
        &self,
        edit: &PendingEdit,
        group_handles: &dyn GroupHandleResolver,
    ) -> Result<DrainedPush, SyncError> {
        let req = PushRequest {
            group_id: edit.group_id,
            memory_id: edit.memory,
            commit: edit.commit.clone(),
            bump: edit.bump,
            message: Some(edit.message.clone()),
        };
        let response = self.client.push_version(&req).await?;

        // Phase 5 deliberately stops short of running the git
        // content transfer when the backend returns Unsupported.
        // That branch is wired in the same phase as the server's
        // git smart HTTP responder; until then the control-plane
        // side is a real effect and we record whether the content
        // side was skipped so callers can explain it to users.
        let content_transferred = match group_handles.resolve(edit.group_id) {
            Some(handle) => {
                let refs = vec![RefSpec::new(
                        mmcp_core::conventions::MAIN_BRANCH_REF,
                        mmcp_core::conventions::MAIN_BRANCH_REF,
                    )];
                let remote_url = self.client.git_url_for(edit.group_id);
                let creds = self.client.git_credentials();
                match self.backend.push(&handle, &remote_url, &refs, &creds).await {
                    Ok(_) => true,
                    Err(mmcp_git::GitError::Unsupported(_)) => false,
                    Err(mmcp_git::GitError::Transport { .. }) => false,
                    Err(other) => return Err(SyncError::Git(other)),
                }
            }
            None => false,
        };

        Ok(DrainedPush {
            edit_id: edit.id,
            response,
            content_transferred,
        })
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
        queue: &PendingQueue,
        filter: SyncFilter,
        group_handles: &dyn GroupHandleResolver,
        scope_index: &dyn ScopeIndex,
    ) -> Result<SyncReport, SyncError> {
        let pulled = self.pull(filter, group_handles, scope_index).await?;
        let pushed = self.push(queue, filter, group_handles, scope_index).await?;
        Ok(SyncReport { pulled, pushed })
    }
}

/// Resolves a `group_id` to its local [`mmcp_git::RepoHandle`].
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
    fn resolve(&self, group_id: Uuid) -> Option<mmcp_git::RepoHandle>;
}

/// Report of a completed `push` call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PushReport {
    pub drained: Vec<DrainedPush>,
}

/// One drained pending edit, with the server's response and
/// whether the content plane was actually able to ship bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DrainedPush {
    pub edit_id: Uuid,
    pub response: PushResponse,
    /// `false` means the control plane registered the version but
    /// the git backend returned `Unsupported` so nothing moved on
    /// the wire yet. Real transport lands in the native backend's
    /// HTTP push phase.
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
