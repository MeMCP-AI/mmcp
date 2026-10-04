//! [`AuthRateLimitConfig`], the per-client budgets of the registration and sign-in routes.

use std::fmt;
use std::num::NonZeroU32;

use super::defaults::{
    DEFAULT_AUTH_RATE_LIMIT_BURST, DEFAULT_AUTH_RATE_LIMIT_IPV6_CLIENT_PREFIX_LENGTH,
    DEFAULT_AUTH_RATE_LIMIT_IPV6_NETWORK_PREFIX_LENGTH, DEFAULT_AUTH_RATE_LIMIT_PER_MINUTE,
    IPV6_NETWORK_QUOTA_MULTIPLE,
};
use super::ipv6_network_tier_config::Ipv6NetworkTierConfig;

/// How fast one client may call the registration and sign-in routes.
///
/// One budget per client is shared across those routes.
/// It refills at [`AuthRateLimitConfig::per_minute`] requests per minute up to [`AuthRateLimitConfig::burst`] requests.
/// A client is its IPv4 address, or the network of its IPv6 address with [`AuthRateLimitConfig::ipv6_client_prefix_length`] leading bits.
/// An optional second budget counts the IPv6 clients of one larger network together.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuthRateLimitConfig {
    /// Requests per minute a client budget refills at.
    pub per_minute: NonZeroU32,
    /// Requests a client may send at once, before the refill rate applies.
    pub burst: NonZeroU32,
    /// Leading bits of an IPv6 address that identify one client, between 1 and 128.
    pub ipv6_client_prefix_length: u8,
    /// The IPv6 network budget, `None` when it is switched off.
    pub ipv6_network_tier: Option<Ipv6NetworkTierConfig>,
}

/// `value` as a non-zero count, one when it is zero.
/// Every caller passes a constant above zero, so the fallback is a safe floor and never a silent change.
pub(super) fn non_zero_or_one(value: u32) -> NonZeroU32 {
    NonZeroU32::new(value).unwrap_or(NonZeroU32::MIN)
}

/// `quota` times [`IPV6_NETWORK_QUOTA_MULTIPLE`], the default quota of the IPv6 network budget.
pub(super) fn default_network_quota(quota: NonZeroU32) -> NonZeroU32 {
    non_zero_or_one(quota.get().saturating_mul(IPV6_NETWORK_QUOTA_MULTIPLE))
}

impl AuthRateLimitConfig {
    /// The compiled-in prefix lengths around a client quota of `per_minute` and `burst`.
    /// The IPv6 network budget gets its default quota, [`IPV6_NETWORK_QUOTA_MULTIPLE`] times the client quota.
    #[must_use]
    pub fn with_client_quota(per_minute: NonZeroU32, burst: NonZeroU32) -> Self {
        Self {
            per_minute,
            burst,
            ipv6_client_prefix_length: DEFAULT_AUTH_RATE_LIMIT_IPV6_CLIENT_PREFIX_LENGTH,
            ipv6_network_tier: Some(Ipv6NetworkTierConfig {
                prefix_length: DEFAULT_AUTH_RATE_LIMIT_IPV6_NETWORK_PREFIX_LENGTH,
                per_minute: default_network_quota(per_minute),
                burst: default_network_quota(burst),
            }),
        }
    }
}

impl Default for AuthRateLimitConfig {
    /// The compiled-in limits, the last tier of the cascade.
    fn default() -> Self {
        Self::with_client_quota(
            non_zero_or_one(DEFAULT_AUTH_RATE_LIMIT_PER_MINUTE),
            non_zero_or_one(DEFAULT_AUTH_RATE_LIMIT_BURST),
        )
    }
}

impl fmt::Display for AuthRateLimitConfig {
    /// Every budget with its quota, for the start log.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "per client (IPv4 address, IPv6 /{prefix}): {per_minute} per minute, burst {burst}",
            prefix = self.ipv6_client_prefix_length,
            per_minute = self.per_minute,
            burst = self.burst,
        )?;
        match self.ipv6_network_tier {
            Some(network) => write!(
                formatter,
                "; per IPv6 /{prefix} network: {per_minute} per minute, burst {burst}",
                prefix = network.prefix_length,
                per_minute = network.per_minute,
                burst = network.burst,
            ),
            None => formatter.write_str("; no IPv6 network budget"),
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn default_network_budget_allows_ten_times_the_client_quota() {
        let config = AuthRateLimitConfig::default();
        let network = config.ipv6_network_tier.unwrap();
        assert_eq!(config.per_minute.get(), 20);
        assert_eq!(config.burst.get(), 10);
        assert_eq!(config.ipv6_client_prefix_length, 64);
        assert_eq!(network.prefix_length, 48);
        assert_eq!(network.per_minute.get(), 200);
        assert_eq!(network.burst.get(), 100);
    }

    #[test]
    fn network_quota_follows_the_client_quota_and_saturates() {
        let small = AuthRateLimitConfig::with_client_quota(
            NonZeroU32::new(3).unwrap(),
            NonZeroU32::new(2).unwrap(),
        );
        let network = small.ipv6_network_tier.unwrap();
        assert_eq!((network.per_minute.get(), network.burst.get()), (30, 20));

        let huge = AuthRateLimitConfig::with_client_quota(NonZeroU32::MAX, NonZeroU32::MAX);
        assert_eq!(huge.ipv6_network_tier.unwrap().per_minute, NonZeroU32::MAX);
    }

    #[test]
    fn display_lists_every_budget_with_its_quota() {
        assert_eq!(
            AuthRateLimitConfig::default().to_string(),
            "per client (IPv4 address, IPv6 /64): 20 per minute, burst 10; per IPv6 /48 network: 200 per minute, burst 100"
        );
        let without_network = AuthRateLimitConfig {
            ipv6_network_tier: None,
            ..AuthRateLimitConfig::default()
        };
        assert_eq!(
            without_network.to_string(),
            "per client (IPv4 address, IPv6 /64): 20 per minute, burst 10; no IPv6 network budget"
        );
    }
}
