//! Background sweep of the auth rate limiter.

use std::convert::Infallible;
use std::sync::Arc;

use super::auth_rate_limiter::AuthRateLimiter;
use super::defaults::AUTH_RATE_LIMIT_SWEEP_INTERVAL;
use crate::background::{PeriodicTaskHandle, spawn_periodic_task};

/// Name under which the sweep reports a failed tick.
const SWEEPER_TASK_NAME: &str = "auth rate limit sweep";

/// Spawn the sweep that forgets the keys whose budget is full again, in every budget of the limiter.
///
/// Each sweep logs the refusals since the previous one at info level when there were any, and the keys each budget still remembers at debug level.
/// The caller drops the returned handle when the server stops, which aborts the sweep.
pub fn spawn_auth_rate_limit_sweeper(rate_limiter: Arc<AuthRateLimiter>) -> PeriodicTaskHandle {
    spawn_periodic_task(
        SWEEPER_TASK_NAME,
        AUTH_RATE_LIMIT_SWEEP_INTERVAL,
        move || {
            let rate_limiter = rate_limiter.clone();
            async move {
                let report = rate_limiter.sweep();
                if report.refused_requests > 0 {
                    tracing::info!(
                        refused_requests = report.refused_requests,
                        retained_keys = report.retained_keys(),
                        "auth rate limit refused requests since the last sweep"
                    );
                }
                for retention in &report.retained_per_tier {
                    tracing::debug!(
                        tier = %retention.scope,
                        retained_keys = retention.retained_keys,
                        "auth rate limiter swept"
                    );
                }
                Ok::<(), Infallible>(())
            }
        },
    )
}
