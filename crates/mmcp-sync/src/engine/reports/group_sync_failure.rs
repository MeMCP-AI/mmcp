//! [`GroupSyncFailure`]: one group's failed `push` / `pull` / `fetch` attempt.

use uuid::Uuid;

use crate::error::SyncError;

/// One group's `push` / `pull` / `fetch` attempt that ended in a
/// genuine [`SyncError`] (a git-level failure, a diverged ref, and
/// so on), keeping the failing group's id attached to its error.
///
/// Every group scheduled for an operation is attempted regardless of
/// whether an earlier one (in list order) failed: bounded concurrency
/// already runs every group to completion (see
/// `crate::engine::concurrency::run_bounded`'s doc comment on the
/// index-tag-then-sort pattern), so this type exists purely to carry
/// the FAILED subset's identity and cause forward into the report
/// instead of the whole call collapsing to whichever error happened
/// to be first in list order and silently discarding every group
/// that actually succeeded, earlier or later.
#[derive(Debug)]
pub struct GroupSyncFailure {
    pub group_id: Uuid,
    pub error: SyncError,
}
