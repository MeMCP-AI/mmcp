//! Organization grouping multiple users together.

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::id::OrgId;

/// An mmcp organization.
///
/// Owns groups and is the unit of cross-user collaboration. Mirrors
/// the GitHub org concept. An org has zero or more user members and
/// zero or more groups under its namespace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Org {
    /// Stable identifier, generated at org creation.
    pub id: OrgId,

    /// Unique slug used in URLs and qualified group names. Lower-case,
    /// dash-separated.
    pub slug: String,

    /// Optional human display name.
    pub display_name: Option<String>,

    /// When the org was first created.
    pub created_at: Timestamp,
}
