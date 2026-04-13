//! Symbolic reference to a group in a load set.

use serde::{Deserialize, Serialize};

use crate::id::ProjectUuid;

/// A group referenced by name, before the server resolves it to a
/// concrete [`GroupId`](crate::id::GroupId).
///
/// Clients build their effective load set from these references,
/// then hand the list to the server (or to local cache resolution in
/// offline mode) to get the actual group records.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "value")]
pub enum GroupRef {
    /// The caller's personal `global` group.
    Global,

    /// The group keyed by a project's UUID identity.
    Project(ProjectUuid),

    /// A language convention group under the `lang/` namespace.
    Language(String),

    /// Any other group referenced by its canonical name
    /// (e.g. `team-acme/shared`).
    Named(String),
}

impl GroupRef {
    /// Canonical name used in URLs and server API calls.
    #[must_use]
    pub fn canonical_name(&self) -> String {
        match self {
            GroupRef::Global => "global".to_string(),
            GroupRef::Project(uuid) => uuid.to_string(),
            GroupRef::Language(lang) => format!("lang/{lang}"),
            GroupRef::Named(name) => name.clone(),
        }
    }
}
