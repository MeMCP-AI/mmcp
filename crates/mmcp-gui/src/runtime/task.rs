//! Messages sent from the UI thread to the background worker.
//!
//! Each variant is an intent: "refresh the group index", "load the
//! slug list for this group", "load this memory body", "pull from
//! remote", "run diagnostics", "create / update / delete this
//! memory". The worker translates intents into `mmcp-store` /
//! `mmcp-sync` calls and posts results back as
//! [`crate::runtime::outcome::TaskOutcome`].

use mmcp_core::id::GroupId;
use mmcp_core::memory::MemoryFile;

#[derive(Debug, Clone)]
pub enum BackgroundTask {
    RefreshGroups,
    LoadMemoryList {
        group_id: GroupId,
    },
    LoadMemory {
        group_id: GroupId,
        slug: String,
    },
    SyncPull,
    SyncPush,
    RunDiagnose,
    CreateMemory {
        group_id: GroupId,
        slug: String,
        memory: MemoryFile,
    },
    UpdateMemory {
        group_id: GroupId,
        slug: String,
        memory: MemoryFile,
    },
    DeleteMemory {
        group_id: GroupId,
        slug: String,
    },
}
