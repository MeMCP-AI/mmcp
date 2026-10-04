//! [`AuthRateLimitSweepReport`], what one sweep of the limiter found.

/// The outcome of [`crate::rate_limit::AuthRateLimiter::sweep`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuthRateLimitSweepReport {
    /// Clients whose budget is not full again, so the limiter still remembers them.
    pub retained_clients: usize,
    /// Requests refused since the previous sweep.
    pub refused_requests: u64,
}
