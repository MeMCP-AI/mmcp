//! Newtype wrapping a [`Uuid`] to identify a [`User`](crate::identity::User).

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Stable identifier for a [`User`](crate::identity::User).
///
/// Always a UUIDv7 so that chronologically ordered iteration matches
/// account creation order.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct UserId(Uuid);

impl UserId {
    /// Generate a fresh `UserId` using a UUIDv7 timestamp.
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }

    /// Wrap an existing UUID without generating a new one.
    ///
    /// Useful when reading an identifier from storage or a wire format.
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

impl Default for UserId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for UserId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}
