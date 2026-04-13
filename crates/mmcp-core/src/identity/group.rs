//! Memory group: the unit of storage and permissioning.

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::id::{GroupId, OrgId, UserId};

/// An mmcp memory group.
///
/// One group corresponds to exactly one git repository. The group's
/// owner is either a user (personal group) or an org (org-owned
/// group), captured by [`GroupOwner`].
///
/// Groups hold zero or more memories and have their own ACL rules
/// resolved against the workspace identity model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Group {
    /// Stable identifier, generated at group creation.
    pub id: GroupId,

    /// Slug unique within the owner's namespace. Used in URLs and
    /// when resolving qualified group references like `acme/shared`.
    pub slug: String,

    /// Owner of the group.
    pub owner: GroupOwner,

    /// Optional human display name shown in WebUI listings.
    pub display_name: Option<String>,

    /// When the group was first created.
    pub created_at: Timestamp,
}

/// Identifies the principal that owns a [`Group`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "id")]
pub enum GroupOwner {
    /// Personal group owned by a single user.
    User(UserId),

    /// Org-owned group, governed by the org's membership and ACLs.
    Org(OrgId),
}
