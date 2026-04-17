//! Outcome of an ACL resolution call.

use serde::{Deserialize, Serialize};

use crate::identity::Role;

/// Effective role for a principal on a group.
///
/// `None` means the principal has no access at all. Any `Some(role)`
/// value is the maximum role reached by any membership path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EffectiveRole(Option<Role>);

impl EffectiveRole {
    /// No access at all.
    #[must_use]
    pub const fn none() -> Self {
        Self(None)
    }

    /// Access at exactly the given role.
    #[must_use]
    pub const fn of(role: Role) -> Self {
        Self(Some(role))
    }

    /// Retrieve the inner role, if any.
    #[must_use]
    pub const fn role(self) -> Option<Role> {
        self.0
    }

    /// True if the principal holds at least `required`.
    #[must_use]
    pub fn allows(self, required: Role) -> bool {
        matches!(self.0, Some(r) if r.includes(required))
    }

    /// Return the stronger of `self` and `other`.
    #[must_use]
    pub fn max(self, other: Self) -> Self {
        match (self.0, other.0) {
            (None, None) => Self::none(),
            (Some(a), None) => Self::of(a),
            (None, Some(b)) => Self::of(b),
            (Some(a), Some(b)) => Self::of(a.max(b)),
        }
    }
}

impl Default for EffectiveRole {
    fn default() -> Self {
        Self::none()
    }
}

impl From<Option<Role>> for EffectiveRole {
    fn from(role: Option<Role>) -> Self {
        Self(role)
    }
}
