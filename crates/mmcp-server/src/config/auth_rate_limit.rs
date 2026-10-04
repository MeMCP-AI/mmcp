//! [`AuthRateLimitConfig`], the per-client budget of the unauthenticated auth routes.

use std::num::NonZeroU32;

use super::defaults::{DEFAULT_AUTH_RATE_LIMIT_BURST, DEFAULT_AUTH_RATE_LIMIT_PER_MINUTE};

/// How fast one client may call the unauthenticated auth routes.
///
/// One budget per client is shared across those routes.
/// It refills at [`AuthRateLimitConfig::per_minute`] requests per minute up to [`AuthRateLimitConfig::burst`] requests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuthRateLimitConfig {
    /// Requests per minute the budget refills at.
    pub per_minute: NonZeroU32,
    /// Requests a client may send at once, before the refill rate applies.
    pub burst: NonZeroU32,
}

/// `value` as a non-zero count, one when it is zero.
/// Every caller passes a constant above zero, so the fallback is a safe floor and never a silent change.
pub(super) fn non_zero_or_one(value: u32) -> NonZeroU32 {
    NonZeroU32::new(value).unwrap_or(NonZeroU32::MIN)
}

impl Default for AuthRateLimitConfig {
    /// The compiled-in limits, the last tier of the cascade.
    fn default() -> Self {
        Self {
            per_minute: non_zero_or_one(DEFAULT_AUTH_RATE_LIMIT_PER_MINUTE),
            burst: non_zero_or_one(DEFAULT_AUTH_RATE_LIMIT_BURST),
        }
    }
}
