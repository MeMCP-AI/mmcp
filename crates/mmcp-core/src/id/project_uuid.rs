//! Newtype wrapping a [`Uuid`] for the per-project identity stored in
//! `.mmcp/config.toml`.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Stable identity for an mmcp-managed project.
///
/// Generated once by `mmcp init` (locally if offline, or accepted by
/// the server on first push) and never changes for the life of the
/// project. Survives directory renames, slug changes, and forks.
///
/// `ProjectUuid` is the canonical key the server uses to map a working
/// directory to its memory group.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct ProjectUuid(Uuid);

impl ProjectUuid {
    /// Generate a fresh `ProjectUuid` using a UUIDv7 timestamp.
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

impl Default for ProjectUuid {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for ProjectUuid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}
