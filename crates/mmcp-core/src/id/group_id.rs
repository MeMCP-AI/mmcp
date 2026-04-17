//! Newtype wrapping a [`Uuid`] to identify a [`Group`](crate::identity::Group).

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Stable identifier for a [`Group`](crate::identity::Group).
///
/// UUIDv7 so creation-time ordering matches lexicographic ordering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct GroupId(Uuid);

impl GroupId {
    /// Generate a fresh `GroupId` using a UUIDv7 timestamp.
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

impl Default for GroupId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for GroupId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}
