//! Role granted to a principal on a group.

use serde::{Deserialize, Serialize};

/// Permission level granted to a principal on a [`Group`](crate::identity::Group).
///
/// Roles are ordered by escalating capability:
///
/// 1. [`Read`](Role::Read) - pull and read memories.
/// 2. [`Write`](Role::Write) - additionally push edits.
/// 3. [`Admin`](Role::Admin) - additionally manage the group's ACL.
///
/// Effective roles are computed by [`acl`](crate::acl) (forthcoming),
/// which takes the maximum across every membership path the principal
/// has into the group.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// Read-only: pull and inspect memories.
    Read = 0,

    /// Read plus push: edit and create memories in the group.
    Write = 1,

    /// Read plus write plus ACL management.
    Admin = 2,
}

impl Role {
    /// True if this role grants at least the capability of `other`.
    #[must_use]
    pub fn includes(self, other: Role) -> bool {
        self >= other
    }
}
