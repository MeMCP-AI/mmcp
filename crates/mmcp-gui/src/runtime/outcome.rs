//! Messages sent from the background worker back to the UI thread.
//!
//! Errors are flattened into a pre-rendered `String` so the UI side
//! doesn't have to pattern-match on non-`Clone` error variants each
//! frame; the background worker formats them once at the failure
//! site. Sync failures are distinguished from generic failures via
//! [`TaskOutcome::SyncFailed`] so the UI can route them to the
//! sync status bar instead of the toast queue.

use std::sync::Arc;

use mmcp_core::id::GroupId;
use mmcp_core::memory::MemoryFile;
use mmcp_store::{DiagReport, GroupEntry};

use crate::state::sync_status::SyncOp;

#[derive(Debug)]
pub enum TaskOutcome {
    GroupsRefreshed(Vec<GroupEntry>),
    MemoryListLoaded {
        group_id: GroupId,
        slugs: Vec<String>,
    },
    MemoryLoaded {
        group_id: GroupId,
        slug: String,
        memory: Arc<MemoryFile>,
    },
    /// Reported once at worker startup. `server_url = None` means
    /// sync is not configured for this project and the UI should
    /// keep the Pull / Push buttons disabled.
    SyncAvailable {
        server_url: Option<String>,
    },
    SyncPullCompleted {
        updated: usize,
        new_groups: usize,
    },
    SyncPushCompleted {
        drained: usize,
    },
    SyncFailed {
        op: SyncOp,
        message: String,
    },
    /// Emitted by the periodic health probe each tick. `reason` is
    /// the transport error string when `online == false`, or `None`
    /// on success.
    HealthChanged {
        online: bool,
        reason: Option<String>,
    },
    DiagnoseCompleted(DiagReport),
    MemoryCreated {
        group_id: GroupId,
        slug: String,
    },
    MemoryUpdated {
        group_id: GroupId,
        slug: String,
    },
    MemoryDeleted {
        group_id: GroupId,
        slug: String,
    },
    Error(String),
}
