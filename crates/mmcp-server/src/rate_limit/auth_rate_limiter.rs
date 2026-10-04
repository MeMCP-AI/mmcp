//! [`AuthRateLimiter`], the per-client budget of the unauthenticated auth routes.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use axum::http::HeaderMap;
use governor::clock::{Clock, MonotonicClock};
use governor::middleware::NoOpMiddleware;
use governor::state::keyed::DefaultKeyedStateStore;
use governor::{Quota, RateLimiter};

use super::auth_rate_limit_refusal::AuthRateLimitRefusal;
use super::auth_rate_limit_sweep_report::AuthRateLimitSweepReport;
use super::client_address_resolver::ClientAddressResolver;
use super::client_key::ClientKey;
use super::defaults::MIN_RETRY_AFTER_SECONDS;
use crate::config::AuthRateLimitConfig;

/// The keyed limiter over [`ClientKey`] on the clock `C`, with no middleware.
type KeyedLimiter<C> = RateLimiter<
    ClientKey,
    DefaultKeyedStateStore<ClientKey>,
    C,
    NoOpMiddleware<<C as Clock>::Instant>,
>;

/// Seconds a refused client waits, `wait` rounded up and never below [`MIN_RETRY_AFTER_SECONDS`].
/// The limiter reports a zero wait once the earliest time has passed, so the floor is load bearing.
fn retry_after_seconds(wait: Duration) -> u64 {
    let whole_seconds = wait.as_secs();
    let rounded_up = if wait.subsec_nanos() > 0 {
        whole_seconds.saturating_add(1)
    } else {
        whole_seconds
    };
    rounded_up.max(MIN_RETRY_AFTER_SECONDS)
}

/// One request budget per client, shared by every route it guards.
///
/// The budget refills at the configured rate up to the configured burst (GCRA, as `governor` implements it).
/// The clock is a parameter so tests run on a fake clock.
pub struct AuthRateLimiter<C: Clock = MonotonicClock> {
    limiter: KeyedLimiter<C>,
    resolver: ClientAddressResolver,
    refused_requests: AtomicU64,
}

impl AuthRateLimiter<MonotonicClock> {
    /// A limiter on the monotonic system clock.
    #[must_use]
    pub fn new(config: &AuthRateLimitConfig, resolver: ClientAddressResolver) -> Self {
        Self::with_clock(config, resolver, MonotonicClock)
    }
}

impl<C: Clock> AuthRateLimiter<C> {
    /// A limiter measuring time with `clock`.
    #[must_use]
    pub fn with_clock(
        config: &AuthRateLimitConfig,
        resolver: ClientAddressResolver,
        clock: C,
    ) -> Self {
        let quota = Quota::per_minute(config.per_minute).allow_burst(config.burst);
        Self {
            limiter: RateLimiter::dashmap_with_clock(quota, clock),
            resolver,
            refused_requests: AtomicU64::new(0),
        }
    }

    /// Spend one request of the budget of the client that sent a request with `headers` from `peer`.
    ///
    /// # Errors
    /// [`AuthRateLimitRefusal::TooManyRequests`] when the client spent its budget.
    /// [`AuthRateLimitRefusal::PeerAddressMissing`] when `peer` is absent, since no client can be identified.
    pub fn check(
        &self,
        headers: &HeaderMap,
        peer: Option<SocketAddr>,
    ) -> Result<(), AuthRateLimitRefusal> {
        let peer = peer.ok_or(AuthRateLimitRefusal::PeerAddressMissing)?;
        let client = ClientKey::from_client_address(self.resolver.resolve(headers, peer));
        self.limiter.check_key(&client).map_err(|not_until| {
            self.refused_requests.fetch_add(1, Ordering::Relaxed);
            let wait = not_until.wait_time_from(self.limiter.clock().now());
            AuthRateLimitRefusal::TooManyRequests {
                client,
                retry_after_seconds: retry_after_seconds(wait),
            }
        })
    }

    /// Forget the clients whose budget has been full for a replenishment interval and report what is left.
    ///
    /// Such a client is indistinguishable from one never seen, so forgetting it changes no answer.
    /// The memory held is therefore bounded by the clients active within one full replenishment, one replenishment interval and one sweep period.
    pub fn sweep(&self) -> AuthRateLimitSweepReport {
        self.limiter.retain_recent();
        self.limiter.shrink_to_fit();
        AuthRateLimitSweepReport {
            retained_clients: self.limiter.len(),
            refused_requests: self.refused_requests.swap(0, Ordering::Relaxed),
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use std::num::NonZeroU32;

    use governor::clock::FakeRelativeClock;

    use super::*;
    use crate::config::TrustedProxies;

    const SLOW_PER_MINUTE: u32 = 20;
    const FAST_PER_MINUTE: u32 = 60;
    const PEER_PORT: u16 = 40_000;
    const CLIENT: &str = "203.0.113.10";
    const OTHER_CLIENT: &str = "203.0.113.11";

    fn config(per_minute: u32, burst: u32) -> AuthRateLimitConfig {
        AuthRateLimitConfig {
            per_minute: NonZeroU32::new(per_minute).unwrap(),
            burst: NonZeroU32::new(burst).unwrap(),
        }
    }

    fn limiter(
        per_minute: u32,
        burst: u32,
        trusted_proxies: &str,
    ) -> (AuthRateLimiter<FakeRelativeClock>, FakeRelativeClock) {
        let clock = FakeRelativeClock::default();
        let resolver = ClientAddressResolver::new(&TrustedProxies::parse(trusted_proxies));
        (
            AuthRateLimiter::with_clock(&config(per_minute, burst), resolver, clock.clone()),
            clock,
        )
    }

    fn peer(address: &str) -> Option<SocketAddr> {
        Some(SocketAddr::new(address.parse().unwrap(), PEER_PORT))
    }

    fn check(limiter: &AuthRateLimiter<FakeRelativeClock>, address: &str) -> Result<(), u64> {
        match limiter.check(&HeaderMap::new(), peer(address)) {
            Ok(()) => Ok(()),
            Err(AuthRateLimitRefusal::TooManyRequests {
                retry_after_seconds,
                ..
            }) => Err(retry_after_seconds),
            Err(other) => panic!("unexpected refusal {other:?}"),
        }
    }

    #[test]
    fn retry_after_rounds_up_and_is_at_least_one_second() {
        assert_eq!(retry_after_seconds(Duration::ZERO), 1);
        assert_eq!(retry_after_seconds(Duration::from_nanos(1)), 1);
        assert_eq!(retry_after_seconds(Duration::from_millis(999)), 1);
        assert_eq!(retry_after_seconds(Duration::from_secs(1)), 1);
        assert_eq!(retry_after_seconds(Duration::from_millis(1001)), 2);
        assert_eq!(retry_after_seconds(Duration::from_secs(3)), 3);
        assert_eq!(retry_after_seconds(Duration::from_millis(2500)), 3);
    }

    #[test]
    fn requests_beyond_the_burst_are_refused_with_the_wait_rounded_up() {
        let (limiter, clock) = limiter(SLOW_PER_MINUTE, 2, "");
        assert_eq!(check(&limiter, CLIENT), Ok(()));
        assert_eq!(check(&limiter, CLIENT), Ok(()));
        // 20 requests per minute replenish one request every three seconds.
        assert_eq!(check(&limiter, CLIENT), Err(3));

        clock.advance(Duration::from_millis(500));
        assert_eq!(check(&limiter, CLIENT), Err(3), "2.5 seconds round up to 3");

        clock.advance(Duration::from_millis(2500));
        assert_eq!(check(&limiter, CLIENT), Ok(()));
    }

    #[test]
    fn a_refusal_that_waits_a_fraction_of_a_second_still_says_one_second() {
        let (limiter, clock) = limiter(FAST_PER_MINUTE, 1, "");
        assert_eq!(check(&limiter, CLIENT), Ok(()));
        clock.advance(Duration::from_millis(500));
        assert_eq!(check(&limiter, CLIENT), Err(1));
    }

    #[test]
    fn each_client_has_its_own_budget() {
        let (limiter, _clock) = limiter(SLOW_PER_MINUTE, 1, "");
        assert_eq!(check(&limiter, CLIENT), Ok(()));
        assert!(check(&limiter, CLIENT).is_err());
        assert_eq!(check(&limiter, OTHER_CLIENT), Ok(()));
    }

    #[test]
    fn ipv6_addresses_in_one_slash_64_draw_on_one_budget() {
        let (limiter, _clock) = limiter(SLOW_PER_MINUTE, 1, "");
        assert_eq!(check(&limiter, "2001:db8:1:2::1"), Ok(()));
        assert!(check(&limiter, "2001:db8:1:2:ffff::9").is_err());
        assert_eq!(check(&limiter, "2001:db8:1:3::1"), Ok(()));
    }

    #[test]
    fn clients_behind_a_trusted_proxy_have_separate_budgets() {
        let (limiter, _clock) = limiter(SLOW_PER_MINUTE, 1, "10.0.0.0/24");
        let behind_proxy = |forwarded: &str| {
            let mut headers = HeaderMap::new();
            headers.insert("x-forwarded-for", forwarded.parse().unwrap());
            limiter.check(&headers, peer("10.0.0.2"))
        };
        assert_eq!(behind_proxy(CLIENT), Ok(()));
        assert!(behind_proxy(CLIENT).is_err());
        assert_eq!(behind_proxy(OTHER_CLIENT), Ok(()));
    }

    #[test]
    fn x_forwarded_for_from_an_untrusted_peer_does_not_renew_its_budget() {
        let (limiter, _clock) = limiter(SLOW_PER_MINUTE, 1, "10.0.0.0/24");
        let direct = |forwarded: &str| {
            let mut headers = HeaderMap::new();
            headers.insert("x-forwarded-for", forwarded.parse().unwrap());
            limiter.check(&headers, peer(CLIENT))
        };
        assert_eq!(direct("198.51.100.1"), Ok(()));
        assert!(direct("198.51.100.2").is_err());
    }

    #[test]
    fn a_request_without_a_peer_address_is_refused_and_never_counted() {
        let (limiter, _clock) = limiter(SLOW_PER_MINUTE, 1, "");
        assert_eq!(
            limiter.check(&HeaderMap::new(), None),
            Err(AuthRateLimitRefusal::PeerAddressMissing)
        );
        assert_eq!(check(&limiter, CLIENT), Ok(()), "no key was spent");
        assert_eq!(limiter.sweep().refused_requests, 0);
    }

    #[test]
    fn sweep_drops_clients_back_at_a_fresh_state_and_keeps_active_ones() {
        let (limiter, clock) = limiter(FAST_PER_MINUTE, 2, "");
        assert_eq!(check(&limiter, CLIENT), Ok(()));
        assert_eq!(check(&limiter, OTHER_CLIENT), Ok(()));
        assert_eq!(check(&limiter, OTHER_CLIENT), Ok(()));

        // One request replenishes per second.
        // The limiter drops a client once its budget has been full for one replenishment interval.
        // CLIENT spent one request and is dropped after two seconds, OTHER_CLIENT spent two and is still kept then.
        clock.advance(Duration::from_secs(2));
        assert_eq!(limiter.sweep().retained_clients, 1);

        clock.advance(Duration::from_secs(1));
        assert_eq!(limiter.sweep().retained_clients, 0);
    }

    #[test]
    fn sweep_reports_the_refusals_since_the_previous_sweep() {
        let (limiter, _clock) = limiter(SLOW_PER_MINUTE, 1, "");
        assert_eq!(check(&limiter, CLIENT), Ok(()));
        assert!(check(&limiter, CLIENT).is_err());
        assert!(check(&limiter, CLIENT).is_err());

        assert_eq!(limiter.sweep().refused_requests, 2);
        assert_eq!(limiter.sweep().refused_requests, 0);
    }

    #[test]
    fn a_swept_client_is_served_as_if_never_seen() {
        let (limiter, clock) = limiter(SLOW_PER_MINUTE, 1, "");
        assert_eq!(check(&limiter, CLIENT), Ok(()));
        clock.advance(Duration::from_secs(3));
        limiter.sweep();
        assert_eq!(check(&limiter, CLIENT), Ok(()));
        assert!(check(&limiter, CLIENT).is_err());
    }
}
