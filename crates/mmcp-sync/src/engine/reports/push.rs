//! [`PushReport`] and its per-group item [`PushedGroup`].

use uuid::Uuid;

use super::GroupSyncFailure;

/// Report of a completed `push` call.
///
/// `SyncError` is not `Clone`/`PartialEq` (it wraps `mmcp_git::GitError`,
/// which wraps `std::io::Error`), so this report does not derive
/// those either; nothing in this workspace compares or clones a
/// `PushReport` as a whole (field-level assertions cover the tests).
#[derive(Debug)]
pub struct PushReport {
    /// Groups whose push attempt did NOT error, in iteration order.
    /// A group appears here even when the content plane was
    /// skipped (see `PushedGroup::content_transferred`); a group
    /// whose push attempt itself errored appears in `failed`
    /// instead, never here.
    pub pushed: Vec<PushedGroup>,
    /// Groups whose push attempt itself errored (not merely a
    /// content-plane transport skip, which still counts as a
    /// successful `PushedGroup` with `content_transferred: false`).
    /// Every OTHER scheduled group still ran to completion regardless
    /// of a group appearing here; see [`GroupSyncFailure`].
    pub failed: Vec<GroupSyncFailure>,
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
