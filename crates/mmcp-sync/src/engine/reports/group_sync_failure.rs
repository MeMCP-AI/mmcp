//! [`GroupSyncFailure`]: one group's failed `push` / `pull` / `fetch` attempt.

use mmcp_core::id::GroupId;

use crate::error::SyncError;

/// One group's `push` / `pull` / `fetch` attempt that ended in a genuine [`SyncError`] (a git-level failure,
/// a diverged ref, and so on).
/// Keeps the failing group's id attached to its error.
///
/// `group_id` is the [`GroupId`] newtype rather than a bare `Uuid`,
/// matching this workspace's identifier convention (see [`mmcp_core::id::GroupId`]).
/// This prevents mixing up a group id with an unrelated UUID (a memory id, a user id, ...).
///
/// Every group scheduled for an operation is attempted regardless of an earlier group's outcome.
/// See also: `crate::engine::concurrency::run_bounded` for the completion-order guarantee that makes this safe.
///
/// This type carries only the FAILED subset's identity and cause into the report.
/// It avoids collapsing to whichever error happened first, discarding every group that actually succeeded.
#[derive(Debug)]
pub struct GroupSyncFailure {
    pub group_id: GroupId,
    pub error: SyncError,
}
