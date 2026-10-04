//! [`TierRetention`], how many keys one tier still remembers after a sweep.

use super::tier_scope::TierScope;

/// The keys one tier of the limiter still holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TierRetention {
    /// The tier that was swept.
    pub scope: TierScope,
    /// Keys whose budget is not full again, so the tier still remembers them.
    pub retained_keys: usize,
}
