//! Version record for a memory.

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::id::{MemoryId, UserId};

/// Canonical version entry for a memory at a point in time.
///
/// One `Version` corresponds to exactly one commit in the group's
/// underlying git repository. The `number` field is assigned by the
/// server when the commit lands on the canonical branch; offline
/// commits carry a [`BumpIntent`](crate::memory::BumpIntent) until
/// they are pushed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Version {
    /// Memory this version belongs to.
    pub memory: MemoryId,

    /// Semver assigned by the server when the version was published.
    pub number: semver::Version,

    /// Commit hash in the group's git repository. Kept as a string
    /// because it may be SHA-1 or SHA-256 depending on the repo.
    pub commit: String,

    /// Principal that authored the edit.
    pub author: UserId,

    /// When the commit was published to the canonical branch.
    pub published_at: Timestamp,

    /// Optional human-readable note describing the change. Shown in
    /// the WebUI history view and in diff summaries.
    pub summary: Option<String>,
}
