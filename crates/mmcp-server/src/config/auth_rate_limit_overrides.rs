//! [`AuthRateLimitOverrides`], the argument parser's tier of the auth rate limit settings.

/// The auth rate limit settings the argument parser resolved, each the flag or else its environment variable.
///
/// Every value is kept as text.
/// A blank value counts as absent, which is what Compose interpolation of an unset `${VAR:-}` produces.
/// A malformed value warns and falls through to the next tier instead of stopping the start.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AuthRateLimitOverrides {
    /// `--auth-rate-limit-per-minute`, `MMCP_AUTH_RATE_LIMIT_PER_MINUTE`.
    pub per_minute: Option<String>,
    /// `--auth-rate-limit-burst`, `MMCP_AUTH_RATE_LIMIT_BURST`.
    pub burst: Option<String>,
    /// `--auth-rate-limit-ipv6-client-prefix-length`, `MMCP_AUTH_RATE_LIMIT_IPV6_CLIENT_PREFIX_LENGTH`.
    pub ipv6_client_prefix_length: Option<String>,
    /// `--auth-rate-limit-ipv6-network-prefix-length`, `MMCP_AUTH_RATE_LIMIT_IPV6_NETWORK_PREFIX_LENGTH`.
    /// `0` switches the IPv6 network budget off.
    pub ipv6_network_prefix_length: Option<String>,
    /// `--auth-rate-limit-ipv6-network-per-minute`, `MMCP_AUTH_RATE_LIMIT_IPV6_NETWORK_PER_MINUTE`.
    pub ipv6_network_per_minute: Option<String>,
    /// `--auth-rate-limit-ipv6-network-burst`, `MMCP_AUTH_RATE_LIMIT_IPV6_NETWORK_BURST`.
    pub ipv6_network_burst: Option<String>,
}
