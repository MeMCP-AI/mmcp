//! [`AuthRateLimitSweepReport`], what one sweep of the limiter found.

use super::tier_retention::TierRetention;

/// The outcome of [`crate::rate_limit::AuthRateLimiter::sweep`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthRateLimitSweepReport {
    /// The keys each tier still remembers, in the order the tiers are checked.
    pub retained_per_tier: Vec<TierRetention>,
    /// Requests refused since the previous sweep.
    pub refused_requests: u64,
}

impl AuthRateLimitSweepReport {
    /// The keys remembered by every tier together.
    #[must_use]
    pub fn retained_keys(&self) -> usize {
        self.retained_per_tier
            .iter()
            .map(|retention| retention.retained_keys)
            .sum()
    }
}
