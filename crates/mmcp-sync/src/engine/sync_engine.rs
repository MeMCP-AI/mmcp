//! [`SyncEngine`]: push/pull/fetch/sync orchestration bound to a
//! fixed list of [`BoundRemote`]s.

use std::collections::HashSet;
use std::sync::Arc;

use mmcp_core::id::GroupId;
use mmcp_git::{FastForwardOutcome, GitBackend, RefSpec};
use uuid::Uuid;

use super::concurrency::run_bounded;
use super::defaults::MAX_CONCURRENT_GROUP_TRANSFERS;
use super::push_scope::PushScope;
use super::remote::{BoundRemote, RemoteTransport};
use super::reports::{
    FetchReport, FetchedGroup, GroupSyncFailure, PullReport, PushReport, PushedGroup,
    RemoteManifestFailure, RemotePushOutcome, SyncReport,
};
use super::resolver::{GroupHandleResolver, IndexContended};
use super::scope::group_matches;
use crate::client::RemoteGroup;
use crate::error::SyncError;
use crate::filter::{ScopeIndex, SyncFilter};

/// Split `run_bounded`'s tagged outcomes into successes and
/// [`GroupSyncFailure`]s, attributing each failure to its own group
/// id. `push`, `pull`'s fast-forward step, and `fetch` all reduce
/// their `run_bounded` output this same way.
fn partition_sync_outcomes<R>(
    outcomes: Vec<(Uuid, Result<R, SyncError>)>,
) -> (Vec<R>, Vec<GroupSyncFailure>) {
    let mut successes = Vec::with_capacity(outcomes.len());
    let mut failed = Vec::new();
    for (group_id, outcome) in outcomes {
        match outcome {
            Ok(value) => successes.push(value),
            Err(error) => failed.push(GroupSyncFailure {
                group_id: GroupId::from_uuid(group_id),
                error,
            }),
        }
    }
    (successes, failed)
}

/// One candidate group queued for `fetch_one_group`, tagging the
/// bound remote and control-plane metadata (when any) alongside the
/// local handle already resolved for it.
struct FetchCandidate<'a> {
    remote: &'a BoundRemote,
    group_id: Uuid,
    slug: Option<String>,
    remote_head: Option<String>,
    handle: mmcp_git::RepoHandle,
}

/// Engine bound to a git backend and a fixed list of remotes.
///
/// Cheap to clone: the backend is already reference-counted
/// internally, and `BoundRemote` is a plain owned struct cloned in
/// full (its own `SyncClient`, when present, is itself
/// reference-counted).
#[derive(Clone)]
pub struct SyncEngine {
    backend: Arc<dyn GitBackend>,
    remotes: Vec<BoundRemote>,
}

impl SyncEngine {
    #[must_use]
    pub fn new(backend: Arc<dyn GitBackend>, remotes: Vec<BoundRemote>) -> Self {
        Self { backend, remotes }
    }

    /// The engine's bound remotes, in construction order. Exposed so
    /// a caller can inspect what an engine was actually wired with
    /// (tests, diagnostics) without re-deriving it from config.
    #[must_use]
    pub fn remotes(&self) -> &[BoundRemote] {
        &self.remotes
    }

    /// Resolve `scope` against this engine's bound remotes.
    ///
    /// `Default` picks the one remote marked `default: true`, or the
    /// sole remote when exactly one is bound and none is marked
    /// default; zero remotes, or more than one with none marked
    /// default, is [`SyncError::NoDefaultRemote`] (the mmcp-store
    /// resolver guarantees this never happens for a
    /// resolver-built engine). `All` selects every remote whose
    /// `include_in_push_all` is true. `Named` selects the one exact
    /// name match, or [`SyncError::UnknownRemote`].
    fn resolve_scope(&self, scope: &PushScope) -> Result<Vec<&BoundRemote>, SyncError> {
        match scope {
            PushScope::Default => Ok(vec![self.default_remote()?]),
            PushScope::All => Ok(self
                .remotes
                .iter()
                .filter(|r| r.include_in_push_all)
                .collect()),
            PushScope::Named(name) => self
                .remotes
                .iter()
                .find(|r| &r.name == name)
                .map(|r| vec![r])
                .ok_or_else(|| SyncError::UnknownRemote { name: name.clone() }),
        }
    }

    /// The engine's single default remote. See
    /// [`SyncEngine::resolve_scope`]'s `Default` case for the
    /// resolution rule.
    fn default_remote(&self) -> Result<&BoundRemote, SyncError> {
        if let Some(remote) = self.remotes.iter().find(|r| r.default) {
            return Ok(remote);
        }
        match self.remotes.as_slice() {
            [only] => Ok(only),
            _ => Err(SyncError::NoDefaultRemote),
        }
    }

    /// Remote URL and content-plane credentials to use for `group_id`
    /// against `remote`.
    fn remote_endpoint(
        &self,
        remote: &BoundRemote,
        group_id: Uuid,
    ) -> (String, mmcp_git::Credentials) {
        match &remote.transport {
            RemoteTransport::MmcpServer(client) => {
                (client.git_url_for(group_id), client.git_credentials())
            }
            RemoteTransport::DirectGit { url, creds, .. } => (url.clone(), creds.clone()),
        }
    }

    /// Push each in-scope group's local `main` to every remote
    /// `scope` selects.
    ///
    /// Git-symmetric write path. No pending-edit queue: every
    /// memory mutation already commits to the local bare repo, so
    /// push just walks the selected (remote, group) pairs and runs
    /// `git push <remote> main` on each one.
    ///
    /// `filter` selects which groups are candidates for an
    /// `mmcp-server`-transport remote (`SyncFilter::Group` pushes
    /// only that group, `SyncFilter::All` / `SyncFilter::Scope`
    /// iterate every locally-indexed group filtered by scope). A
    /// `direct-git`-transport remote's only candidate is ever its
    /// own bound `group_id`, intersected with `filter`; when the
    /// filter excludes that group, the remote contributes nothing to
    /// the report, not an error - see `push_one_group`.
    ///
    /// `scope` selects which remotes are attempted at all; see
    /// [`PushScope`]. Transport-layer failures (backend reports
    /// `Unsupported` or `Transport`) are recorded as
    /// `content_transferred: false` rather than raised, so partial
    /// network gaps surface in the report instead of aborting the
    /// whole run. Any other git error (including a diverged push)
    /// lands in the returned report's `failed` field, attributed to
    /// its own group, rather than aborting every other in-flight
    /// group's push.
    pub async fn push(
        &self,
        filter: SyncFilter,
        scope: PushScope,
        group_handles: &dyn GroupHandleResolver,
        scope_index: &dyn ScopeIndex,
    ) -> Result<PushReport, SyncError> {
        let targets = self.resolve_scope(&scope)?;

        let mut by_remote = Vec::with_capacity(targets.len());
        for remote in targets {
            let candidate_groups: Vec<Uuid> = match &remote.transport {
                RemoteTransport::MmcpServer(_) => match filter {
                    SyncFilter::Group(uuid) => vec![uuid],
                    SyncFilter::All | SyncFilter::Scope(_) => group_handles.iter_group_ids(),
                },
                // A direct-git remote is scoped to exactly one
                // group; `push_one_group` still re-checks `filter`
                // below so an out-of-scope bound group contributes
                // nothing rather than erroring.
                RemoteTransport::DirectGit { group_id, .. } => vec![*group_id],
            };

            // Bounded concurrency via `run_bounded`: each group's
            // network round trip is independent, so up to
            // `MAX_CONCURRENT_GROUP_TRANSFERS` run at once, with
            // `candidate_groups`' original order preserved in the
            // result regardless of completion order.
            let outcomes = run_bounded(
                candidate_groups,
                MAX_CONCURRENT_GROUP_TRANSFERS,
                |group_id| *group_id,
                |group_id| async move {
                    self.push_one_group(remote, group_id, filter, group_handles, scope_index)
                        .await
                },
            )
            .await;

            let (pushed, failed) = partition_sync_outcomes(outcomes);
            // `push_one_group` reports `Ok(None)` for an out-of-scope
            // group and `Err(GroupNotIndexed | GroupIndexContended)`
            // (already folded into `failed` above) for an unresolved
            // one; keep only the concrete pushes here.
            let pushed = pushed.into_iter().flatten().collect();
            by_remote.push(RemotePushOutcome {
                remote_name: remote.name.clone(),
                pushed,
                failed,
            });
        }

        Ok(PushReport { by_remote })
    }

    /// Push one group's local `main` to one remote. `Ok(None)` means
    /// the group was out of `filter`'s scope, the ordinary case for a
    /// broad `SyncFilter::All`/`SyncFilter::Scope` candidate list. A
    /// group whose local repo handle could not be resolved is a
    /// genuine, reportable failure instead, split by cause: see
    /// [`SyncError::GroupNotIndexed`] (removed group, or an explicitly
    /// named but never-indexed group) and
    /// [`SyncError::GroupIndexContended`] (transient index race).
    async fn push_one_group(
        &self,
        remote: &BoundRemote,
        group_id: Uuid,
        filter: SyncFilter,
        group_handles: &dyn GroupHandleResolver,
        scope_index: &dyn ScopeIndex,
    ) -> Result<Option<PushedGroup>, SyncError> {
        if !group_matches(filter, group_id, scope_index) {
            return Ok(None);
        }
        let handle = match group_handles.resolve(group_id) {
            Ok(Some(handle)) => handle,
            Ok(None) => {
                // Visible instead of a silent drop: the group was a
                // real candidate (enumerated by `iter_group_ids` or
                // explicitly named by `filter`) moments before this
                // lookup ran, so a genuinely unindexed group here is
                // worth an operator's attention even though every
                // other scheduled group still completes.
                tracing::warn!(
                    group = %group_id,
                    remote = %remote.name,
                    "push skipped: group not indexed locally"
                );
                return Err(SyncError::GroupNotIndexed { group: group_id });
            }
            Err(IndexContended) => {
                // Same visibility rationale as the not-indexed arm
                // above, but transient: the caller can retry this
                // group without re-indexing anything.
                tracing::warn!(
                    group = %group_id,
                    remote = %remote.name,
                    "push skipped: local index lookup was contended"
                );
                return Err(SyncError::GroupIndexContended { group: group_id });
            }
        };
        let refs = vec![RefSpec::new(
            mmcp_core::conventions::MAIN_BRANCH_REF,
            mmcp_core::conventions::MAIN_BRANCH_REF,
        )];
        let (remote_url, creds) = self.remote_endpoint(remote, group_id);
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
                // raised (see `SyncEngine::push`'s doc comment), so
                // this line is the only signal a caller gets that
                // bytes did not ship.
                tracing::warn!(
                    group = %group_id,
                    remote = %remote.name,
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
    /// local `main` to match the DEFAULT remote only.
    ///
    /// Git-symmetric two-phase operation:
    ///
    /// 1. Delegate to `self.fetch`, which writes every in-scope
    ///    remote's remote head into its own
    ///    `refs/remotes/<name>/main` without touching local `main`.
    /// 2. For each group the DEFAULT remote fetched, call
    ///    `backend.fast_forward(main, refs/remotes/<default>/main)`
    ///    to move local `main` forward.
    ///
    /// This is the one asymmetry between `fetch` (broad,
    /// informational, every applicable remote) and `pull` (narrow,
    /// mutating): every OTHER bound remote's tracking ref still gets
    /// written by the `fetch` phase and stays inspectable, but never
    /// auto-advances local `main`. Advancing from more than one
    /// remote's view would make "which remote's history is my local
    /// `main` actually built from" an unanswerable question.
    ///
    /// Divergence (local is not an ancestor of remote) is captured,
    /// not silent: a `FastForwardOutcome::NotFastForward` from the
    /// backend is raised as the structured `SyncError::PullDiverged`
    /// and lands in the returned report's `failed` field, attributed
    /// to its own group. Local `main` is left unchanged for that
    /// group; every other in-flight group still completes normally.
    ///
    /// `filter` semantics match `fetch`. A remote whose manifest poll
    /// itself failed during the `fetch` phase never aborts `pull`
    /// either: its entry carries forward unchanged into the returned
    /// report's `manifest_failures` (see [`RemoteManifestFailure`]
    /// and `fetch`'s own doc comment), while every other remote's
    /// fetched groups still fast-forward normally when they belong to
    /// the default remote.
    pub async fn pull(
        &self,
        filter: SyncFilter,
        group_handles: &dyn GroupHandleResolver,
        scope_index: &dyn ScopeIndex,
    ) -> Result<PullReport, SyncError> {
        let default_remote_name = self.default_remote()?.name.clone();
        let fetched = self.fetch(filter, group_handles, scope_index).await?;
        // Groups that already failed during the fetch phase (see
        // `fetch`'s own `failed` field) carry forward into this
        // report unchanged: `fetched.groups` only ever contains the
        // fetches that actually succeeded, so the fast-forward loop
        // below never re-attempts or silently drops them.
        let mut failed = fetched.failed;

        // Only the default remote's fetched entries ever advance
        // local `main`; see this method's own doc comment.
        let to_advance: Vec<FetchedGroup> = fetched
            .groups
            .into_iter()
            .filter(|g| g.remote_name == default_remote_name)
            .collect();

        // Bounded concurrency via `run_bounded` instead of one
        // fast-forward after another; see `concurrency::run_bounded`'s
        // doc comment for the index-tag-then-sort ordering rationale.
        // Same group-id tagging as `push` so a fast-forward failure
        // is attributed to its own group instead of discarding every
        // other group's already-advanced result.
        let outcomes = run_bounded(
            to_advance,
            MAX_CONCURRENT_GROUP_TRANSFERS,
            |fetched_group| fetched_group.group_id,
            |fetched_group| {
                // `.ok().flatten()`: this call site does not yet
                // distinguish contended-vs-not-indexed (see
                // `SyncError::GroupIndexContended`/`GroupNotIndexed`
                // on the `push` path); both collapse to `None` here,
                // matching this path's prior behavior exactly.
                let handle = group_handles.resolve(fetched_group.group_id).ok().flatten();
                async move { self.fast_forward_one_group(fetched_group, handle).await }
            },
        )
        .await;

        let (updated, new_failed) = partition_sync_outcomes(outcomes);
        failed.extend(new_failed);
        Ok(PullReport {
            updated,
            new_groups: fetched.new_groups,
            failed,
            manifest_failures: fetched.manifest_failures,
        })
    }

    /// Fast-forward one group's local `main` to its already-fetched
    /// tracking ref, then return its `RemoteGroup` summary regardless
    /// of whether a fast-forward actually ran.
    async fn fast_forward_one_group(
        &self,
        fetched_group: FetchedGroup,
        handle: Option<mmcp_git::RepoHandle>,
    ) -> Result<RemoteGroup, SyncError> {
        let group_id = fetched_group.group_id;
        let slug = fetched_group.slug.unwrap_or_else(|| group_id.to_string());
        // Overwritten below by the fast-forward's own real outcome
        // whenever one runs; this is only the fallback used when the
        // tracking ref never advanced (transport skip) or the
        // remote never advertised a head (a `direct-git` remote,
        // which has no control-plane manifest to advertise one
        // from).
        let mut head_commit = fetched_group.remote_head.unwrap_or_default();

        // Only attempt the fast-forward when the tracking ref
        // actually moved. `ref_updated=false` means the content
        // plane was skipped (transport failure), so the tracking ref
        // still points at whatever prior fetch left it at -
        // advancing from that is harmless at best and misleading at
        // worst.
        if fetched_group.ref_updated
            && let Some(handle) = handle
        {
            let target_ref = format!("refs/remotes/{}/main", fetched_group.remote_name);
            match self
                .backend
                .fast_forward(
                    &handle,
                    mmcp_core::conventions::MAIN_BRANCH_REF,
                    &target_ref,
                )
                .await
            {
                // Use the fast-forward's own real result, not the
                // manifest-advertised head: the two normally agree
                // for an `mmcp-server` remote, and this is the ONLY
                // source of truth at all for a `direct-git` remote.
                Ok(FastForwardOutcome::AlreadyAt { commit }) => head_commit = commit,
                Ok(FastForwardOutcome::Advanced { to, .. }) => head_commit = to,
                // Divergence: local has commits the remote does not.
                // Git-symmetric `git pull --ff-only` failure. Raised
                // so the operator can resolve.
                Ok(FastForwardOutcome::NotFastForward { local, target }) => {
                    return Err(SyncError::PullDiverged {
                        group: group_id,
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
        Ok(RemoteGroup {
            group_id,
            slug,
            head_commit,
        })
    }

    /// Pull then push. Push runs with `PushScope::Default`: a full
    /// `sync` targets only the default remote, matching `pull`'s own
    /// single-remote scope; a caller that wants a broader push runs
    /// `push` directly with an explicit [`PushScope`].
    pub async fn sync(
        &self,
        filter: SyncFilter,
        group_handles: &dyn GroupHandleResolver,
        scope_index: &dyn ScopeIndex,
    ) -> Result<SyncReport, SyncError> {
        let pulled = self.pull(filter, group_handles, scope_index).await?;
        let pushed = self
            .push(filter, PushScope::Default, group_handles, scope_index)
            .await?;
        Ok(SyncReport { pulled, pushed })
    }

    /// Fetch every applicable remote's view of each in-scope group
    /// into that remote's own local remote-tracking ref, without
    /// advancing `refs/heads/main`.
    ///
    /// Git-symmetric read-only sync, now aggregated across remotes:
    /// every `mmcp-server`-transport remote's manifest is polled
    /// (`new_groups` deduplicates by `group_id` across every
    /// manifest, so a group advertised by two remotes appears once),
    /// and for each resolved, in-scope group the native backend
    /// writes `refs/remotes/<remote_name>/main` per remote so
    /// operators can inspect any remote's incoming tip before `pull`
    /// fast-forwards from the default one. A `direct-git`-transport
    /// remote has no manifest to poll; its only candidate is ever its
    /// own bound `group_id`, fetched when that group already
    /// resolves locally and matches `filter`.
    ///
    /// One remote's manifest poll failing never aborts the others: an
    /// unreachable remote's error lands in the returned report's
    /// `manifest_failures` (see [`RemoteManifestFailure`]) instead of
    /// propagating via `?`, and every OTHER `mmcp-server` remote is
    /// still polled and still contributes its groups normally. Two
    /// bound remotes, one down, means the caller still gets the
    /// reachable one's full result rather than an all-or-nothing
    /// `Err` that discards it too.
    pub async fn fetch(
        &self,
        filter: SyncFilter,
        group_handles: &dyn GroupHandleResolver,
        scope_index: &dyn ScopeIndex,
    ) -> Result<FetchReport, SyncError> {
        // Poll every mmcp-server remote's manifest, collecting each
        // one's own outcome rather than propagating the first
        // failure: a manifest read is a quick control-plane call, but
        // with several bound remotes an unreachable one must not
        // narrow which groups the OTHER remotes still get to
        // advertise. See `RemoteManifestFailure` and this method's
        // own doc comment.
        let mut manifests: Vec<(&BoundRemote, crate::client::ManifestResponse)> = Vec::new();
        let mut manifest_failures: Vec<RemoteManifestFailure> = Vec::new();
        for remote in &self.remotes {
            if let RemoteTransport::MmcpServer(client) = &remote.transport {
                match client.get_manifest().await {
                    Ok(manifest) => manifests.push((remote, manifest)),
                    Err(error) => manifest_failures.push(RemoteManifestFailure {
                        remote_name: remote.name.clone(),
                        error,
                    }),
                }
            }
        }

        let mut new_groups = Vec::new();
        let mut seen_new_groups: HashSet<Uuid> = HashSet::new();
        let mut candidates: Vec<FetchCandidate<'_>> = Vec::new();
        for (remote, manifest) in &manifests {
            for remote_group in &manifest.groups {
                // `.ok().flatten()`: see the fast-forward call site
                // above for why contended-vs-not-indexed is not yet
                // distinguished on this discovery path.
                match group_handles.resolve(remote_group.group_id).ok().flatten() {
                    None => {
                        if seen_new_groups.insert(remote_group.group_id) {
                            new_groups.push(remote_group.clone());
                        }
                    }
                    Some(handle) => {
                        if group_matches(filter, remote_group.group_id, scope_index) {
                            candidates.push(FetchCandidate {
                                remote,
                                group_id: remote_group.group_id,
                                slug: Some(remote_group.slug.clone()),
                                remote_head: Some(remote_group.head_commit.clone()),
                                handle,
                            });
                        }
                    }
                }
            }
        }

        // The direct-git remote (at most one bound group each) has
        // no manifest to discover new groups from; its only ever
        // candidate is its own bound group, when already resolved
        // locally and in scope.
        for remote in &self.remotes {
            if let RemoteTransport::DirectGit { group_id, .. } = &remote.transport
                && let Some(handle) = group_handles.resolve(*group_id).ok().flatten()
                && group_matches(filter, *group_id, scope_index)
            {
                candidates.push(FetchCandidate {
                    remote,
                    group_id: *group_id,
                    slug: None,
                    remote_head: None,
                    handle,
                });
            }
        }

        // Bounded concurrency via `run_bounded` instead of one fetch
        // after another; see `concurrency::run_bounded`'s doc comment
        // for the index-tag-then-sort ordering rationale. Same
        // group-id tagging as `push` so one candidate's fetch failure
        // is attributed to it specifically instead of discarding
        // every other candidate's already-succeeded fetch.
        let outcomes = run_bounded(
            candidates,
            MAX_CONCURRENT_GROUP_TRANSFERS,
            |candidate| candidate.group_id,
            |candidate| async move { self.fetch_one_group(candidate).await },
        )
        .await;

        let (groups, failed) = partition_sync_outcomes(outcomes);
        Ok(FetchReport {
            groups,
            new_groups,
            failed,
            manifest_failures,
        })
    }

    /// Fetch one candidate's remote head into its remote's own local
    /// tracking ref.
    async fn fetch_one_group(
        &self,
        candidate: FetchCandidate<'_>,
    ) -> Result<FetchedGroup, SyncError> {
        // `refs/heads/main:refs/remotes/<remote_name>/main` - the
        // git-native shape for "inspect before apply" fetches, now
        // per remote instead of a single shared `origin`. Local
        // `main` stays put; `pull` fast-forwards it from the
        // default remote's tracking ref only.
        let target_ref = candidate.remote.tracking_ref();
        let refs = vec![RefSpec::new(
            mmcp_core::conventions::MAIN_BRANCH_REF,
            target_ref,
        )];
        let (remote_url, creds) = self.remote_endpoint(candidate.remote, candidate.group_id);
        let ref_updated = match self
            .backend
            .fetch(&candidate.handle, &remote_url, &refs, &creds)
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
            group_id: candidate.group_id,
            remote_name: candidate.remote.name.clone(),
            slug: candidate.slug,
            remote_head: candidate.remote_head,
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
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::stderr_indicates_non_fast_forward;
    use super::*;
    use std::sync::Arc;

    fn server_remote(name: &str, default: bool, include_in_push_all: bool) -> BoundRemote {
        let client =
            crate::client::SyncClient::new("https://mmcp.example.com").expect("build client");
        BoundRemote {
            name: name.to_string(),
            default,
            include_in_push_all,
            transport: RemoteTransport::MmcpServer(client),
        }
    }

    /// `resolve_scope` / `default_remote` never touch `self.backend`,
    /// so a real (but never-opened) `NativeBackend` rooted in a
    /// throwaway tempdir stands in here; a hand-rolled `GitBackend`
    /// fake would need the crate to take on an `async_trait` /
    /// `bytes` dependency purely for unreachable trait-method stubs.
    fn engine_with(remotes: Vec<BoundRemote>) -> (SyncEngine, tempfile::TempDir) {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let backend = mmcp_git::NativeBackend::new(tmp.path()).expect("backend");
        (SyncEngine::new(Arc::new(backend), remotes), tmp)
    }

    #[test]
    fn resolve_scope_default_picks_the_marked_remote() {
        let (engine, _tmp) = engine_with(vec![
            server_remote("primary", true, true),
            server_remote("mirror", false, true),
        ]);
        let targets = engine.resolve_scope(&PushScope::Default).expect("resolve");
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].name, "primary");
    }

    #[test]
    fn resolve_scope_default_falls_back_to_the_sole_remote() {
        let (engine, _tmp) = engine_with(vec![server_remote("only", false, true)]);
        let targets = engine.resolve_scope(&PushScope::Default).expect("resolve");
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].name, "only");
    }

    #[test]
    fn resolve_scope_default_with_no_remotes_is_no_default_remote() {
        let (engine, _tmp) = engine_with(vec![]);
        let err = engine
            .resolve_scope(&PushScope::Default)
            .expect_err("must fail with zero remotes");
        assert!(matches!(err, SyncError::NoDefaultRemote));
    }

    #[test]
    fn resolve_scope_default_with_two_remotes_and_no_default_is_no_default_remote() {
        let (engine, _tmp) = engine_with(vec![
            server_remote("a", false, true),
            server_remote("b", false, true),
        ]);
        let err = engine
            .resolve_scope(&PushScope::Default)
            .expect_err("ambiguous default must error");
        assert!(matches!(err, SyncError::NoDefaultRemote));
    }

    #[test]
    fn resolve_scope_all_only_selects_remotes_opted_in() {
        let (engine, _tmp) = engine_with(vec![
            server_remote("a", true, true),
            server_remote("b", false, false),
        ]);
        let targets = engine.resolve_scope(&PushScope::All).expect("resolve");
        let names: Vec<&str> = targets.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, vec!["a"]);
    }

    #[test]
    fn resolve_scope_named_finds_the_exact_match() {
        let (engine, _tmp) = engine_with(vec![
            server_remote("a", true, true),
            server_remote("b", false, true),
        ]);
        let targets = engine
            .resolve_scope(&PushScope::Named("b".to_string()))
            .expect("resolve");
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].name, "b");
    }

    #[test]
    fn resolve_scope_named_with_no_match_is_unknown_remote() {
        let (engine, _tmp) = engine_with(vec![server_remote("a", true, true)]);
        let err = engine
            .resolve_scope(&PushScope::Named("ghost".to_string()))
            .expect_err("unknown name must error");
        assert!(matches!(err, SyncError::UnknownRemote { name } if name == "ghost"));
    }

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
