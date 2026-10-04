//! Constants of the auth rate limit that no configuration layer overrides.

use std::time::Duration;

/// Period of the sweep that drops the keys whose budget is full again.
/// A key that stays silent for a full replenishment is back at a fresh state, so the period only bounds how long its entry stays in memory.
pub(crate) const AUTH_RATE_LIMIT_SWEEP_INTERVAL: Duration = Duration::from_secs(60);

/// Smallest `Retry-After` the limiter ever sends, in seconds.
/// A refusal whose wait rounds to zero would invite an immediate retry, so the answer is never below one.
pub(super) const MIN_RETRY_AFTER_SECONDS: u64 = 1;
