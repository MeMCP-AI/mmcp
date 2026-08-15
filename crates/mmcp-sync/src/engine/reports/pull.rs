//! [`PullReport`].

use super::GroupSyncFailure;

/// Report of a completed `pull` call.
#[derive(Debug)]
pub struct PullReport {
    /// Groups whose local head was advanced (or is already in sync).
    pub updated: Vec<crate::client::RemoteGroup>,
    /// Groups the server says exist but the client has no local
    /// clone for yet.
    pub new_groups: Vec<crate::client::RemoteGroup>,
    /// Groups that failed either during the fetch phase or the
    /// fast-forward phase. See [`GroupSyncFailure`] and
    /// [`super::PushReport::failed`]'s doc comment for the same
    /// "every group is still attempted" guarantee.
    pub failed: Vec<GroupSyncFailure>,
}
