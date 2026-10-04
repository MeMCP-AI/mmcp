//! [`AuthRateLimiter`], the per-client budgets of the unauthenticated auth routes.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use axum::http::HeaderMap;
use governor::Quota;
use governor::clock::{Clock, MonotonicClock};

use super::auth_rate_limit_refusal::AuthRateLimitRefusal;
use super::auth_rate_limit_sweep_report::AuthRateLimitSweepReport;
use super::client_address_resolver::ClientAddressResolver;
use super::defaults::MIN_RETRY_AFTER_SECONDS;
use super::rate_limit_tier::RateLimitTier;
use super::tier_scope::TierScope;
use crate::config::AuthRateLimitConfig;

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

/// The request budgets of the unauthenticated auth routes, shared by every route it guards.
///
/// The first budget is one per client, an IPv4 address or an IPv6 network of the configured prefix length.
/// An optional second budget counts the IPv6 clients of one larger network together, with a quota of its own.
/// A budget refills at its configured rate up to its configured burst (GCRA, as `governor` implements it).
/// The clock is a parameter so tests run on a fake clock.
pub struct AuthRateLimiter<C: Clock + Clone = MonotonicClock> {
    /// The budgets in the order they are checked, the client budget first.
    tiers: Vec<RateLimitTier<C>>,
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

impl<C: Clock + Clone> AuthRateLimiter<C> {
    /// A limiter measuring time with `clock`.
    #[must_use]
    pub fn with_clock(
        config: &AuthRateLimitConfig,
        resolver: ClientAddressResolver,
        clock: C,
    ) -> Self {
        let mut tiers = vec![RateLimitTier::new(
            TierScope::Client {
                ipv6_prefix_length: config.ipv6_client_prefix_length,
            },
            Quota::per_minute(config.per_minute).allow_burst(config.burst),
            clock.clone(),
        )];
        if let Some(network) = config.ipv6_network_tier {
            tiers.push(RateLimitTier::new(
                TierScope::Ipv6Network {
                    prefix_length: network.prefix_length,
                },
                Quota::per_minute(network.per_minute).allow_burst(network.burst),
                clock,
            ));
        }
        Self {
            tiers,
            resolver,
            refused_requests: AtomicU64::new(0),
        }
    }

    /// Spend one request of the budgets of the client that sent a request with `headers` from `peer`.
    ///
    /// The client budget is checked first, and a request it refuses never touches the network budget.
    /// A host that spent its own budget therefore cannot use up the budget of the network it shares with others.
    /// The cost is that a client rotating through addresses still adds one entry per request to the client map until the sweep.
    ///
    /// # Errors
    /// [`AuthRateLimitRefusal::TooManyRequests`] when a budget is spent, naming the budget that refused and its wait.
    /// [`AuthRateLimitRefusal::PeerAddressMissing`] when `peer` is absent, since no client can be identified.
    pub fn check(
        &self,
        headers: &HeaderMap,
        peer: Option<SocketAddr>,
    ) -> Result<(), AuthRateLimitRefusal> {
        let peer = peer.ok_or(AuthRateLimitRefusal::PeerAddressMissing)?;
        let address = self.resolver.resolve(headers, peer);
        for tier in &self.tiers {
            tier.check(address).map_err(|refusal| {
                self.refused_requests.fetch_add(1, Ordering::Relaxed);
                AuthRateLimitRefusal::TooManyRequests {
                    client: refusal.client,
                    retry_after_seconds: retry_after_seconds(refusal.wait),
                }
            })?;
        }
        Ok(())
    }

    /// Forget the keys whose budget has been full for a replenishment interval, in every budget, and report what is left.
    ///
    /// Such a key is indistinguishable from one never seen, so forgetting it changes no answer.
    /// The memory held is bounded by the request rate times one replenishment interval plus one sweep period.
    /// It is not bounded by the number of active clients, since every new address adds a key until a sweep drops it.
    pub fn sweep(&self) -> AuthRateLimitSweepReport {
        AuthRateLimitSweepReport {
            retained_per_tier: self.tiers.iter().map(RateLimitTier::sweep).collect(),
            refused_requests: self.refused_requests.swap(0, Ordering::Relaxed),
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use std::num::NonZeroU32;

    use governor::clock::FakeRelativeClock;

    use super::super::client_key::ClientKey;
    use super::*;
    use crate::config::{Ipv6NetworkTierConfig, TrustedProxies};

    const SLOW_PER_MINUTE: u32 = 20;
    const FAST_PER_MINUTE: u32 = 60;
    const PEER_PORT: u16 = 40_000;
    const CLIENT: &str = "203.0.113.10";
    const OTHER_CLIENT: &str = "203.0.113.11";
    const CLIENT_PREFIX: u8 = 64;
    const NETWORK_PREFIX: u8 = 48;
    /// Three addresses in three different /64 of the network `2001:db8:1::/48`.
    const NETWORK_MEMBERS: [&str; 3] = ["2001:db8:1:1::1", "2001:db8:1:2::1", "2001:db8:1:3::1"];
    /// An address in another /48.
    const OUTSIDE_NETWORK: &str = "2001:db8:2:1::1";

    fn non_zero(value: u32) -> NonZeroU32 {
        NonZeroU32::new(value).unwrap()
    }

    /// A client budget alone, with no IPv6 network budget.
    fn config(per_minute: u32, burst: u32) -> AuthRateLimitConfig {
        AuthRateLimitConfig {
            per_minute: non_zero(per_minute),
            burst: non_zero(burst),
            ipv6_client_prefix_length: CLIENT_PREFIX,
            ipv6_network_tier: None,
        }
    }

    /// A client budget and an IPv6 /48 network budget.
    fn config_with_network(client: (u32, u32), network: (u32, u32)) -> AuthRateLimitConfig {
        AuthRateLimitConfig {
            ipv6_network_tier: Some(Ipv6NetworkTierConfig {
                prefix_length: NETWORK_PREFIX,
                per_minute: non_zero(network.0),
                burst: non_zero(network.1),
            }),
            ..config(client.0, client.1)
        }
    }

    type TestLimiter = AuthRateLimiter<FakeRelativeClock>;

    fn limiter_for(
        config: &AuthRateLimitConfig,
        trusted_proxies: &str,
    ) -> (TestLimiter, FakeRelativeClock) {
        let clock = FakeRelativeClock::default();
        let resolver = ClientAddressResolver::new(&TrustedProxies::parse(trusted_proxies));
        (
            AuthRateLimiter::with_clock(config, resolver, clock.clone()),
            clock,
        )
    }

    fn limiter(
        per_minute: u32,
        burst: u32,
        trusted_proxies: &str,
    ) -> (TestLimiter, FakeRelativeClock) {
        limiter_for(&config(per_minute, burst), trusted_proxies)
    }

    fn peer(address: &str) -> Option<SocketAddr> {
        Some(SocketAddr::new(address.parse().unwrap(), PEER_PORT))
    }

    /// The wait in seconds of a refusal, `Ok` when the request is admitted.
    fn check(limiter: &TestLimiter, address: &str) -> Result<(), u64> {
        refused(limiter, address).map_err(|(_, retry_after)| retry_after)
    }

    /// The refusing key and the wait in seconds of a refusal, `Ok` when the request is admitted.
    fn refused(limiter: &TestLimiter, address: &str) -> Result<(), (String, u64)> {
        match limiter.check(&HeaderMap::new(), peer(address)) {
            Ok(()) => Ok(()),
            Err(AuthRateLimitRefusal::TooManyRequests {
                client,
                retry_after_seconds,
            }) => Err((client.to_string(), retry_after_seconds)),
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
        assert_eq!(limiter.sweep().retained_keys(), 1);

        clock.advance(Duration::from_secs(1));
        assert_eq!(limiter.sweep().retained_keys(), 0);
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

    #[test]
    fn ipv6_clients_in_distinct_slash_64_of_one_slash_48_share_the_network_budget() {
        // Each client prefix has a budget of its own to spare, so only the network budget can refuse.
        let (limiter, _clock) = limiter_for(
            &config_with_network((SLOW_PER_MINUTE, 5), (SLOW_PER_MINUTE, 2)),
            "",
        );
        assert_eq!(check(&limiter, NETWORK_MEMBERS[0]), Ok(()));
        assert_eq!(check(&limiter, NETWORK_MEMBERS[1]), Ok(()));

        let (refusing_key, _) = refused(&limiter, NETWORK_MEMBERS[2]).unwrap_err();

        assert_eq!(refusing_key, "2001:db8:1::/48");
        assert_eq!(
            check(&limiter, OUTSIDE_NETWORK),
            Ok(()),
            "another network has a budget of its own"
        );
    }

    #[test]
    fn an_over_budget_slash_64_does_not_spend_its_slash_48_budget() {
        let (limiter, _clock) = limiter_for(
            &config_with_network((SLOW_PER_MINUTE, 1), (SLOW_PER_MINUTE, 3)),
            "",
        );
        assert_eq!(check(&limiter, NETWORK_MEMBERS[0]), Ok(()));
        // Every one of these is refused by the client budget alone.
        for _ in 0..10 {
            let (refusing_key, _) = refused(&limiter, NETWORK_MEMBERS[0]).unwrap_err();
            assert_eq!(refusing_key, "2001:db8:1:1::/64");
        }

        // The network budget of three requests was spent once, so two neighbours still pass.
        assert_eq!(check(&limiter, NETWORK_MEMBERS[1]), Ok(()));
        assert_eq!(check(&limiter, NETWORK_MEMBERS[2]), Ok(()));
        let (refusing_key, _) = refused(&limiter, "2001:db8:1:4::1").unwrap_err();
        assert_eq!(refusing_key, "2001:db8:1::/48");
    }

    #[test]
    fn ipv4_clients_are_never_counted_by_an_ipv6_tier() {
        let (limiter, _clock) = limiter_for(
            &config_with_network((SLOW_PER_MINUTE, 5), (SLOW_PER_MINUTE, 1)),
            "",
        );
        for index in 0..5 {
            assert_eq!(check(&limiter, &format!("203.0.113.{index}")), Ok(()));
        }

        let report = limiter.sweep();
        assert_eq!(report.retained_per_tier.len(), 2);
        assert_eq!(report.retained_per_tier[1].retained_keys, 0);
        assert_eq!(report.retained_per_tier[0].retained_keys, 5);
    }

    #[test]
    fn ipv4_mapped_clients_share_no_network_tier_key() {
        let (limiter, _clock) = limiter_for(
            &config_with_network((SLOW_PER_MINUTE, 5), (SLOW_PER_MINUTE, 1)),
            "",
        );
        // A network key taken from the raw address would put all of them in `::/48` and refuse the second.
        for index in 0..5 {
            assert_eq!(
                check(&limiter, &format!("::ffff:203.0.113.{index}")),
                Ok(())
            );
        }
        assert_eq!(limiter.sweep().retained_per_tier[1].retained_keys, 0);
    }

    #[test]
    fn retry_after_comes_from_the_tier_that_refused() {
        // The client budget refills every second, the network budget every ten seconds.
        let (limiter, _clock) = limiter_for(&config_with_network((FAST_PER_MINUTE, 5), (6, 1)), "");
        assert_eq!(check(&limiter, NETWORK_MEMBERS[0]), Ok(()));

        let (network_key, network_wait) = refused(&limiter, NETWORK_MEMBERS[1]).unwrap_err();
        assert_eq!(network_key, "2001:db8:1::/48");
        assert_eq!(network_wait, 10);

        // The same client spent its own one-second wait only when its own budget refuses.
        let (limiter, _clock) = limiter_for(&config_with_network((FAST_PER_MINUTE, 1), (6, 5)), "");
        assert_eq!(check(&limiter, NETWORK_MEMBERS[0]), Ok(()));
        let (client_key, client_wait) = refused(&limiter, NETWORK_MEMBERS[0]).unwrap_err();
        assert_eq!(client_key, "2001:db8:1:1::/64");
        assert_eq!(client_wait, 1);
    }

    #[test]
    fn sweep_forgets_idle_entries_of_every_tier() {
        let (limiter, clock) = limiter_for(
            &config_with_network((FAST_PER_MINUTE, 2), (FAST_PER_MINUTE, 3)),
            "",
        );
        for member in NETWORK_MEMBERS {
            assert_eq!(check(&limiter, member), Ok(()));
        }
        let before = limiter.sweep();
        assert_eq!(
            before
                .retained_per_tier
                .iter()
                .map(|retention| retention.retained_keys)
                .collect::<Vec<_>>(),
            [3, 1],
            "three client keys and one network key are held while the budgets are not full"
        );

        clock.advance(Duration::from_secs(10));
        let after = limiter.sweep();
        assert_eq!(
            after
                .retained_per_tier
                .iter()
                .map(|retention| retention.retained_keys)
                .collect::<Vec<_>>(),
            [0, 0]
        );
    }

    #[test]
    fn removing_the_network_tier_keeps_the_slash_64_budget_only() {
        let (limiter, _clock) = limiter_for(&config(SLOW_PER_MINUTE, 1), "");
        for member in NETWORK_MEMBERS {
            assert_eq!(check(&limiter, member), Ok(()), "no network budget refuses");
        }
        assert!(check(&limiter, NETWORK_MEMBERS[0]).is_err());
        assert_eq!(limiter.sweep().retained_per_tier.len(), 1);
    }

    #[test]
    fn the_refusing_key_of_a_client_behind_a_trusted_proxy_is_its_forwarded_network() {
        let (limiter, _clock) = limiter_for(
            &config_with_network((SLOW_PER_MINUTE, 5), (SLOW_PER_MINUTE, 1)),
            "10.0.0.0/24",
        );
        let behind_proxy = |forwarded: &str| {
            let mut headers = HeaderMap::new();
            headers.insert("x-forwarded-for", forwarded.parse().unwrap());
            limiter.check(&headers, peer("10.0.0.2"))
        };
        assert_eq!(behind_proxy(NETWORK_MEMBERS[0]), Ok(()));
        match behind_proxy(NETWORK_MEMBERS[1]) {
            Err(AuthRateLimitRefusal::TooManyRequests { client, .. }) => assert_eq!(
                client,
                ClientKey::of_ipv6_network(NETWORK_MEMBERS[1].parse().unwrap(), NETWORK_PREFIX)
                    .unwrap()
            ),
            other => panic!("expected the network budget to refuse, got {other:?}"),
        }
    }
}
