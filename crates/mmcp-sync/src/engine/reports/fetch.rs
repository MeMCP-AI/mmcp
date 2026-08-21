//! [`FetchReport`] and its per-group item [`FetchedGroup`].

use uuid::Uuid;

use super::{GroupSyncFailure, RemoteManifestFailure};

/// Report of a completed `fetch` call.
///
/// Kept flat with a `remote_name` field on [`FetchedGroup`], unlike
/// [`super::PushReport`]'s per-remote `by_remote` nesting: `fetch`
/// always runs against every applicable remote (every `mmcp-server`
/// remote plus the in-scope `direct-git` remote, per
/// [`crate::SyncEngine::fetch`]'s doc comment) rather than a
/// caller-chosen subset, so there is no scope selection to preserve
/// structurally the way `push`'s `PushScope` needs to. A caller that
/// wants one remote's slice filters `groups` by `remote_name`.
#[derive(Debug)]
pub struct FetchReport {
    /// Groups whose remote head was written into their remote's own
    /// `refs/remotes/<name>/main` tracking ref. An empty list means
    /// either no in-scope group was both present locally and
    /// advertised, or every candidate group's fetch attempt itself
    /// errored - that second case still surfaces those groups in
    /// `failed` below, never silently here.
    pub groups: Vec<FetchedGroup>,
    /// Groups an `mmcp-server` remote's manifest advertises that the
    /// client has no local clone for yet, deduplicated by
    /// `group_id` across every polled remote's manifest (a group
    /// advertised by two remotes appears once). Reported so
    /// operators can decide whether to adopt them; the engine never
    /// auto-clones. Only ever populated from a remote whose manifest
    /// poll itself succeeded; a remote in `manifest_failures` below
    /// contributes nothing here.
    pub new_groups: Vec<crate::client::RemoteGroup>,
    /// Groups whose fetch attempt itself errored. See
    /// [`GroupSyncFailure`] and [`super::PushReport`]'s doc comment
    /// for the same "every group is still attempted" guarantee.
    pub failed: Vec<GroupSyncFailure>,
    /// `mmcp-server`-transport remotes whose `/sync/manifest` poll
    /// itself errored, before any group-level candidate could even be
    /// built for that remote. One unreachable remote does not abort
    /// the whole `fetch`: every OTHER remote's manifest is still
    /// polled and its groups still land in `groups` / `new_groups`
    /// above. See [`RemoteManifestFailure`] and
    /// [`crate::SyncEngine::fetch`]'s doc comment.
    pub manifest_failures: Vec<RemoteManifestFailure>,
}

/// One fetched group's before/after snapshot, attributed to the
/// remote it was fetched from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchedGroup {
    pub group_id: Uuid,
    /// Name of the [`crate::BoundRemote`] this group was fetched
    /// from. Feeds `refs/remotes/<remote_name>/main`, the tracking
    /// ref this fetch wrote into, and lets `pull` select only the
    /// entries that came from the default remote.
    pub remote_name: String,
    /// Slug as advertised by the control-plane manifest. `None` for
    /// a `direct-git` remote, which has no manifest to advertise one
    /// from; the engine has no other source for a group's slug.
    pub slug: Option<String>,
    /// Remote HEAD commit as advertised by the control-plane
    /// manifest at the time it was read. `None` for a `direct-git`
    /// remote: after a successful fetch the actual head lives in the
    /// local tracking ref this fetch just wrote, not in an
    /// advertised value - `pull`'s fast-forward step reads it from
    /// there directly rather than through this field.
    pub remote_head: Option<String>,
    /// `false` when the content-plane fetch was skipped (backend
    /// returned `Unsupported` or a transport error). The control-
    /// plane view still shows the remote head so operators can
    /// see what would have landed.
    pub ref_updated: bool,
}
