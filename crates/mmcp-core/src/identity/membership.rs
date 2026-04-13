//! Membership relation linking a principal to a group with a role.

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::id::{GroupId, OrgId, UserId};
use crate::identity::Role;

/// A membership entry granting a principal a [`Role`] on a
/// [`Group`](crate::identity::Group).
///
/// Memberships are additive: a user can hold multiple memberships
/// reaching the same group through different paths (direct, group
/// membership, org membership). The effective role is the maximum
/// across all paths, computed by the ACL resolver.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Membership {
    /// Group the membership grants access to.
    pub group: GroupId,

    /// Principal receiving the access.
    pub principal: Principal,

    /// Role granted by this membership entry.
    pub role: Role,

    /// When the membership was created. Used for audit, not for
    /// permission resolution.
    pub granted_at: Timestamp,
}

/// Principal that can hold a membership.
///
/// Granting a role to an org or to another group propagates to all
/// of that container's members, subject to the resolver's union
/// semantics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "id")]
pub enum Principal {
    /// A single end user.
    User(UserId),

    /// An entire org. Every member of the org inherits the granted role.
    Org(OrgId),

    /// Another group. Every principal with read access on that group
    /// inherits the granted role on the target group.
    Group(GroupId),
}
