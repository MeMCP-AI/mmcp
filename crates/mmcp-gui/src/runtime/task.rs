//! Messages sent from the UI thread to the background worker.
//!
//! Each variant is an intent: "refresh the group index", "load the
//! slug list for this group", "load this memory body". The worker
//! translates intents into `mmcp-store` / `mmcp-git` calls and posts
//! results back as [`crate::runtime::outcome::TaskOutcome`].

use mmcp_core::id::GroupId;

#[derive(Debug, Clone)]
pub enum BackgroundTask {
    RefreshGroups,
    LoadMemoryList { group_id: GroupId },
    LoadMemory { group_id: GroupId, slug: String },
}
