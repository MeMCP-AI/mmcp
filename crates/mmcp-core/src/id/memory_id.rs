//! Newtype wrapping a [`Uuid`] to identify a memory entry.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Stable identifier for a memory entry inside a group.
///
/// Memories are also keyed by `name` within their group repo, but the
/// `MemoryId` is the durable handle that survives renames and moves
/// between groups.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MemoryId(Uuid);

impl MemoryId {
    /// Generate a fresh `MemoryId` using a UUIDv7 timestamp.
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }

    /// Wrap an existing UUID without generating a new one.
    #[must_use]
    pub const fn from_uuid(uuid: Uuid) -> Self {
        Self(uuid)
    }

    /// Borrow the inner UUID.
    #[must_use]
    pub const fn as_uuid(&self) -> &Uuid {
        &self.0
    }
}

impl Default for MemoryId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for MemoryId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}
