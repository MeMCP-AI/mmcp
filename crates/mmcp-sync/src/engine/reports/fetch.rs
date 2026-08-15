//! [`FetchReport`] and its per-group item [`FetchedGroup`].

use uuid::Uuid;

use super::GroupSyncFailure;

/// Report of a completed `fetch` call.
#[derive(Debug)]
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
    /// Groups whose fetch attempt itself errored. See
    /// [`GroupSyncFailure`] and [`super::PushReport::failed`]'s doc
    /// comment for the same "every group is still attempted" guarantee.
    pub failed: Vec<GroupSyncFailure>,
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
