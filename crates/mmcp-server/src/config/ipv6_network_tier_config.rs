//! [`Ipv6NetworkTierConfig`], the second auth rate limit that counts a whole IPv6 network.

use std::num::NonZeroU32;

/// The limit on one IPv6 network, checked beside the limit on one client.
///
/// A holder of one network can rotate through every client prefix inside it.
/// This limit bounds that holder as a whole, with a quota larger than a single client's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ipv6NetworkTierConfig {
    /// Leading bits of an IPv6 address that identify one network.
    /// Always shorter than [`crate::config::AuthRateLimitConfig::ipv6_client_prefix_length`].
    pub prefix_length: u8,
    /// Requests per minute the network budget refills at.
    pub per_minute: NonZeroU32,
    /// Requests a network may send at once, before the refill rate applies.
    pub burst: NonZeroU32,
}
