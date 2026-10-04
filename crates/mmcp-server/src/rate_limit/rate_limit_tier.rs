//! [`RateLimitTier`], one keyed budget of the limiter.

use std::net::IpAddr;
use std::time::Duration;

use governor::clock::Clock;
use governor::middleware::NoOpMiddleware;
use governor::state::keyed::DefaultKeyedStateStore;
use governor::{Quota, RateLimiter};

use super::client_key::ClientKey;
use super::tier_retention::TierRetention;
use super::tier_scope::TierScope;

/// The keyed limiter over [`ClientKey`] on the clock `C`, with no middleware.
type KeyedLimiter<C> = RateLimiter<
    ClientKey,
    DefaultKeyedStateStore<ClientKey>,
    C,
    NoOpMiddleware<<C as Clock>::Instant>,
>;

/// Why a tier refused a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct TierRefusal {
    /// The key whose budget is spent, under the refusing tier.
    pub(super) client: ClientKey,
    /// Time until that budget admits one more request.
    pub(super) wait: Duration,
}

/// One budget: a scope saying which clients it counts, and a quota per key.
pub(super) struct RateLimitTier<C: Clock> {
    scope: TierScope,
    limiter: KeyedLimiter<C>,
}

impl<C: Clock> RateLimitTier<C> {
    pub(super) fn new(scope: TierScope, quota: Quota, clock: C) -> Self {
        Self {
            scope,
            limiter: RateLimiter::dashmap_with_clock(quota, clock),
        }
    }

    /// Spend one request of the budget of the client at `address`.
    ///
    /// A client this tier does not count spends nothing.
    pub(super) fn check(&self, address: IpAddr) -> Result<(), TierRefusal> {
        let Some(client) = self.scope.key_of(address) else {
            return Ok(());
        };
        self.limiter
            .check_key(&client)
            .map_err(|not_until| TierRefusal {
                client,
                wait: not_until.wait_time_from(self.limiter.clock().now()),
            })
    }

    /// Forget the keys whose budget has been full for a replenishment interval, and report what is left.
    pub(super) fn sweep(&self) -> TierRetention {
        self.limiter.retain_recent();
        self.limiter.shrink_to_fit();
        TierRetention {
            scope: self.scope,
            retained_keys: self.limiter.len(),
        }
    }
}
