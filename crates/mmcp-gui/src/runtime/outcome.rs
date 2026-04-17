//! Messages sent from the background worker back to the UI thread.
//!
//! Errors are flattened into a pre-rendered `String` so the UI side
//! doesn't have to pattern-match on non-`Clone` error variants each
//! frame; the background worker formats them once at the failure
//! site.

use std::sync::Arc;

use mmcp_core::id::GroupId;
use mmcp_core::memory::MemoryFile;
use mmcp_store::GroupEntry;

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
    Error(String),
}
