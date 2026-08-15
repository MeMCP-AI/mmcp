//! [`SyncEngine`]: push/pull/fetch/sync orchestration bound to a
//! specific git backend and sync client.

use std::sync::Arc;

use mmcp_core::id::GroupId;
use mmcp_git::{GitBackend, RefSpec};
use uuid::Uuid;

use super::concurrency::run_bounded;
use super::reports::{
    FetchReport, FetchedGroup, GroupSyncFailure, PullReport, PushReport, PushedGroup, SyncReport,
};
use super::resolver::GroupHandleResolver;
use super::scope::group_matches;
use crate::client::SyncClient;
use crate::error::SyncError;
use crate::filter::{ScopeIndex, SyncFilter};

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

    /// Effective content-plane git credentials, as configured on
    /// the wrapped [`SyncClient`] via `with_push_credential`.
    /// Exposed so a caller can verify which credential a built
    /// engine actually carries, distinct from the control-plane
    /// bearer used by `/sync/*` requests.
    #[must_use]
    pub fn git_credentials(&self) -> mmcp_git::Credentials {
        self.client.git_credentials()
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
    /// error (including a diverged push) lands in the returned
    /// report's `failed` field, attributed to its own group, rather
    /// than aborting every other in-flight group's push; this
    /// function itself only ever returns `Err` for a failure that
    /// precedes the per-group fan-out (none exist today, but the
    /// `Result` signature is kept for forward compatibility).
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

        // Bounded concurrency via `run_bounded`: each group's network
        // round trip is independent, so up to `MAX_CONCURRENT_GROUP_TRANSFERS`
        // (see `concurrency::run_bounded`'s doc comment) run at once,
        // with `targets`' original order preserved in the result
        // regardless of completion order - this is how `PushReport::pushed`'s
        // documented "in iteration order" contract holds. Every scheduled
        // group is attempted; failures and successes both reach the
        // report, attributed to their own `group_id`.
        let outcomes = run_bounded(
            targets,
            |group_id| *group_id,
            |group_id| async move {
                self.push_one_group(group_id, filter, group_handles, scope_index)
                    .await
            },
        )
        .await;

        let mut pushed = Vec::with_capacity(outcomes.len());
        let mut failed = Vec::new();
        for (group_id, outcome) in outcomes {
            match outcome {
                Ok(Some(group)) => pushed.push(group),
                Ok(None) => {}
                Err(error) => failed.push(GroupSyncFailure {
                    group_id: GroupId::from_uuid(group_id),
                    error,
                }),
            }
        }
        Ok(PushReport { pushed, failed })
    }

    /// Push one group's local `main` to its remote. `Ok(None)` means
    /// the group was out of `filter`'s scope or had no local handle
    /// (index refresh raced with push); the caller skips it rather
    /// than treating either as an error.
    async fn push_one_group(
        &self,
        group_id: Uuid,
        filter: SyncFilter,
        group_handles: &dyn GroupHandleResolver,
        scope_index: &dyn ScopeIndex,
    ) -> Result<Option<PushedGroup>, SyncError> {
        if !group_matches(filter, group_id, scope_index) {
            return Ok(None);
        }
        let Some(handle) = group_handles.resolve(group_id) else {
            return Ok(None);
        };
        let refs = vec![RefSpec::new(
            mmcp_core::conventions::MAIN_BRANCH_REF,
            mmcp_core::conventions::MAIN_BRANCH_REF,
        )];
        let remote_url = self.client.git_url_for(group_id);
        let creds = self.client.git_credentials();
        let content_transferred = match self.backend.push(&handle, &remote_url, &refs, &creds).await
        {
            Ok(_) => true,
            Err(mmcp_git::GitError::Unsupported(_)) => false,
            Err(mmcp_git::GitError::Transport { stderr, .. })
                if stderr_indicates_non_fast_forward(&stderr) =>
            {
                // Git-symmetric signal: remote has commits we do not,
                // `git push` refused to overwrite. Raised structured
                // so the CLI and MCP surfaces can tell the operator
                // to pull first.
                return Err(SyncError::PushDiverged {
                    group: group_id,
                    stderr,
                });
            }
            Err(mmcp_git::GitError::Transport { op, url, stderr }) => {
                // A credential mismatch, a network blip, or a
                // rejecting remote all land here indistinguishably.
                // Reported as content_transferred: false rather than
                // raised (see this method's doc comment), so this
                // line is the only signal a caller gets that bytes
                // did not ship.
                tracing::warn!(
                    group = %group_id,
                    op,
                    url = %url,
                    stderr = %stderr,
                    "push content-plane transport failed, reporting content_transferred=false"
                );
                false
            }
            Err(other) => return Err(SyncError::Git(other)),
        };
        Ok(Some(PushedGroup {
            group_id,
            content_transferred,
        }))
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
    /// Divergence (local is not an ancestor of remote) is captured,
    /// not silent: a `FastForwardOutcome::NotFastForward` from the
    /// backend is raised as the structured `SyncError::PullDiverged`
    /// and lands in the returned report's `failed` field, attributed
    /// to its own group. Local `main` is left unchanged for that
    /// group; every other in-flight group still completes normally.
    ///
    /// `filter` and `new_groups` semantics match `fetch`.
    pub async fn pull(
        &self,
        filter: SyncFilter,
        group_handles: &dyn GroupHandleResolver,
        scope_index: &dyn ScopeIndex,
    ) -> Result<PullReport, SyncError> {
        let fetched = self.fetch(filter, group_handles, scope_index).await?;
        // Groups that already failed during the fetch phase (see
        // `fetch`'s own `failed` field) carry forward into this
        // report unchanged: `fetched.groups` only ever contains the
        // fetches that actually succeeded, so the fast-forward loop
        // below never re-attempts or silently drops them.
        let mut failed = fetched.failed;

        // Bounded concurrency via `run_bounded` instead of one
        // fast-forward after another; see `concurrency::run_bounded`'s
        // doc comment for the index-tag-then-sort ordering rationale.
        // Same group-id tagging as `push` so a fast-forward failure
        // is attributed to its own group instead of discarding every
        // other group's already-advanced result.
        let outcomes = run_bounded(
            fetched.groups,
            |fetched_group| fetched_group.group_id,
            |fetched_group| {
                let handle = group_handles.resolve(fetched_group.group_id);
                async move { self.fast_forward_one_group(fetched_group, handle).await }
            },
        )
        .await;

        let mut updated = Vec::with_capacity(outcomes.len());
        for (group_id, outcome) in outcomes {
            match outcome {
                Ok(group) => updated.push(group),
                Err(error) => failed.push(GroupSyncFailure {
                    group_id: GroupId::from_uuid(group_id),
                    error,
                }),
            }
        }
        Ok(PullReport {
            updated,
            new_groups: fetched.new_groups,
            failed,
        })
    }

    /// Fast-forward one group's local `main` to its already-fetched
    /// tracking ref, then return its `RemoteGroup` summary regardless
    /// of whether a fast-forward actually ran.
    async fn fast_forward_one_group(
        &self,
        fetched_group: FetchedGroup,
        handle: Option<mmcp_git::RepoHandle>,
    ) -> Result<crate::client::RemoteGroup, SyncError> {
        // Only attempt the fast-forward when the tracking ref
        // actually moved. `ref_updated=false` means the content
        // plane was skipped (transport failure), so
        // `refs/remotes/origin/main` still points at whatever prior
        // fetch left it at - advancing from that is harmless at best
        // and misleading at worst.
        if fetched_group.ref_updated
            && let Some(handle) = handle
        {
            match self
                .backend
                .fast_forward(
                    &handle,
                    mmcp_core::conventions::MAIN_BRANCH_REF,
                    mmcp_core::conventions::MAIN_REMOTE_TRACKING_REF,
                )
                .await
            {
                Ok(mmcp_git::FastForwardOutcome::AlreadyAt { .. })
                | Ok(mmcp_git::FastForwardOutcome::Advanced { .. }) => {}
                // Divergence: local has commits the remote does not.
                // Git-symmetric `git pull --ff-only` failure. Raised
                // so the operator can resolve.
                Ok(mmcp_git::FastForwardOutcome::NotFastForward { local, target }) => {
                    return Err(SyncError::PullDiverged {
                        group: fetched_group.group_id,
                        local,
                        target,
                    });
                }
                // Missing tracking ref on first fetch of a
                // freshly-cloned repo: benign, the local `main` is
                // already at the target anyway.
                Err(mmcp_git::GitError::RevNotFound(_)) => {}
                Err(mmcp_git::GitError::Unsupported(_)) => {}
                Err(other) => return Err(SyncError::Git(other)),
            }
        }
        Ok(crate::client::RemoteGroup {
            group_id: fetched_group.group_id,
            slug: fetched_group.slug,
            head_commit: fetched_group.remote_head,
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
        let manifest: crate::client::ManifestResponse = self.client.get_manifest().await?;

        // Split synchronously first: `new_groups` needs no network
        // call, and only the resolved-plus-in-scope subset needs the
        // concurrent fetch below.
        let mut new_groups = Vec::new();
        let mut candidates = Vec::new();
        for remote in manifest.groups {
            match group_handles.resolve(remote.group_id) {
                None => new_groups.push(remote),
                Some(handle) => {
                    if group_matches(filter, remote.group_id, scope_index) {
                        candidates.push((remote, handle));
                    }
                }
            }
        }

        // Bounded concurrency via `run_bounded` instead of one fetch
        // after another; see `concurrency::run_bounded`'s doc comment
        // for the index-tag-then-sort ordering rationale. Same
        // group-id tagging as `push` so one group's fetch failure is
        // attributed to it specifically instead of discarding every
        // other group's already-succeeded fetch.
        let outcomes = run_bounded(
            candidates,
            |(remote, _handle)| remote.group_id,
            |(remote, handle)| async move { self.fetch_one_group(remote, handle).await },
        )
        .await;

        let mut groups = Vec::with_capacity(outcomes.len());
        let mut failed = Vec::new();
        for (group_id, outcome) in outcomes {
            match outcome {
                Ok(group) => groups.push(group),
                Err(error) => failed.push(GroupSyncFailure {
                    group_id: GroupId::from_uuid(group_id),
                    error,
                }),
            }
        }
        Ok(FetchReport {
            groups,
            new_groups,
            failed,
        })
    }

    /// Fetch one group's remote head into its local tracking ref.
    async fn fetch_one_group(
        &self,
        remote: crate::client::RemoteGroup,
        handle: mmcp_git::RepoHandle,
    ) -> Result<FetchedGroup, SyncError> {
        // `refs/heads/main:refs/remotes/origin/main` - the
        // git-native shape for "inspect before apply" fetches. Local
        // `main` stays put; the next `pull` fast-forwards it from
        // this tracking ref.
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
            // Transport gaps are recorded, not raised: the
            // control-plane view is still worth surfacing, and the
            // caller can retry.
            Err(mmcp_git::GitError::Unsupported(_)) | Err(mmcp_git::GitError::Transport { .. }) => {
                false
            }
            Err(other) => return Err(SyncError::Git(other)),
        };
        Ok(FetchedGroup {
            group_id: remote.group_id,
            slug: remote.slug,
            remote_head: remote.head_commit,
            ref_updated,
        })
    }
}

/// Sniff the `git push` stderr for the canonical non-fast-forward
/// signals. Git's output here is stable across versions for the
/// three phrasings below; each one means "remote advanced, rebase
/// / pull / force your way out". Matching case-insensitively so
/// future output tweaks around capitalisation do not miss.
fn stderr_indicates_non_fast_forward(stderr: &str) -> bool {
    let lower = stderr.to_ascii_lowercase();
    lower.contains("non-fast-forward")
        || lower.contains("fetch first")
        || lower.contains("rejected")
}

#[cfg(test)]
mod tests {
    use super::stderr_indicates_non_fast_forward;

    #[test]
    fn stderr_non_fast_forward_message_is_recognised() {
        let stderr = "To http://example.com/g.git\n \
                      ! [rejected]        main -> main (non-fast-forward)\n \
                      error: failed to push some refs to 'http://example.com/g.git'\n \
                      hint: Updates were rejected because the tip of your current branch is behind\n";
        assert!(stderr_indicates_non_fast_forward(stderr));
    }

    #[test]
    fn stderr_fetch_first_variant_is_recognised() {
        // Git surfaces this phrasing for the "non-fast-forward of
        // an unrelated branch" case. We still want to classify it
        // as divergence.
        let stderr = " ! [rejected]   main -> main (fetch first)\n";
        assert!(stderr_indicates_non_fast_forward(stderr));
    }

    #[test]
    fn stderr_without_rejection_markers_is_not_a_divergence() {
        // Generic transport error (wrong URL, TLS failure, etc.)
        // must NOT be classified as divergence.
        let stderr = "fatal: unable to access 'http://127.0.0.1:1/no-such.git/': \
                      Failed to connect to 127.0.0.1 port 1: Connection refused";
        assert!(!stderr_indicates_non_fast_forward(stderr));
    }
}
