//! [`ClientKey`], the identity a client's budget is kept under.

use std::fmt;
use std::net::{IpAddr, Ipv6Addr};

use super::defaults::IPV6_CLIENT_PREFIX_LENGTH;

/// Mask keeping the leading [`IPV6_CLIENT_PREFIX_LENGTH`] bits of an IPv6 address.
const IPV6_CLIENT_PREFIX_MASK: u128 = u128::MAX << (u128::BITS - IPV6_CLIENT_PREFIX_LENGTH);

/// The identity of one client: its IPv4 address, or the /64 network of its IPv6 address.
///
/// An IPv4-mapped IPv6 address keys as the IPv4 address it carries, so one client is one key whichever way its address arrives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ClientKey(IpAddr);

impl ClientKey {
    /// The key of the client at `address`.
    #[must_use]
    pub fn from_client_address(address: IpAddr) -> Self {
        match address.to_canonical() {
            IpAddr::V4(address) => Self(IpAddr::V4(address)),
            IpAddr::V6(address) => Self(IpAddr::V6(Ipv6Addr::from(
                u128::from(address) & IPV6_CLIENT_PREFIX_MASK,
            ))),
        }
    }
}

impl fmt::Display for ClientKey {
    /// The IPv4 address, or the IPv6 network with its prefix length.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            IpAddr::V4(address) => write!(formatter, "{address}"),
            IpAddr::V6(network) => write!(formatter, "{network}/{IPV6_CLIENT_PREFIX_LENGTH}"),
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    fn key(address: &str) -> ClientKey {
        ClientKey::from_client_address(address.parse().unwrap())
    }

    #[test]
    fn ipv6_addresses_in_one_slash_64_share_a_client_key() {
        assert_eq!(
            key("2001:db8:1:2:aaaa:bbbb:cccc:dddd"),
            key("2001:db8:1:2::1")
        );
    }

    #[test]
    fn ipv6_addresses_in_different_slash_64_do_not() {
        assert_ne!(key("2001:db8:1:2::1"), key("2001:db8:1:3::1"));
    }

    #[test]
    fn ipv4_mapped_ipv6_keys_as_its_ipv4_address() {
        assert_eq!(key("::ffff:203.0.113.9"), key("203.0.113.9"));
    }

    #[test]
    fn distinct_ipv4_addresses_keep_distinct_keys() {
        assert_ne!(key("203.0.113.9"), key("203.0.113.10"));
    }

    #[test]
    fn display_shows_the_ipv4_address_or_the_ipv6_network_with_its_prefix() {
        assert_eq!(key("203.0.113.9").to_string(), "203.0.113.9");
        assert_eq!(
            key("2001:db8:1:2:aaaa:bbbb:cccc:dddd").to_string(),
            "2001:db8:1:2::/64"
        );
    }
}
