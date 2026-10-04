//! [`ClientKey`], the identity a budget is kept under.

use std::fmt;
use std::net::IpAddr;

use super::network_prefix::ipv6_network;

/// The identity one budget counts: an IPv4 address, or an IPv6 network with its prefix length.
///
/// An IPv4-mapped IPv6 address keys as the IPv4 address it carries, so one client is one key whichever way its address arrives.
/// A key is only comparable with keys of the same tier, since each tier keeps its own map.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ClientKey {
    /// The IPv4 address, or the IPv6 network address with the bits after the prefix zeroed.
    address: IpAddr,
    /// The prefix length of an IPv6 key, `None` for an IPv4 key.
    ipv6_prefix_length: Option<u8>,
}

impl ClientKey {
    /// The key of the client at `address`: its IPv4 address, or its IPv6 network of `ipv6_prefix_length` bits.
    #[must_use]
    pub fn of_client(address: IpAddr, ipv6_prefix_length: u8) -> Self {
        Self::of_ipv6_network(address, ipv6_prefix_length).unwrap_or_else(|| Self {
            address: address.to_canonical(),
            ipv6_prefix_length: None,
        })
    }

    /// The key of the IPv6 network of `prefix_length` bits that holds `address`.
    ///
    /// `None` for an IPv4 or IPv4-mapped address, which no IPv6 network contains.
    #[must_use]
    pub fn of_ipv6_network(address: IpAddr, prefix_length: u8) -> Option<Self> {
        match address.to_canonical() {
            IpAddr::V4(_) => None,
            IpAddr::V6(address) => Some(Self {
                address: IpAddr::V6(ipv6_network(address, prefix_length)),
                ipv6_prefix_length: Some(prefix_length),
            }),
        }
    }
}

impl fmt::Display for ClientKey {
    /// The IPv4 address, or the IPv6 network with its prefix length.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.ipv6_prefix_length {
            None => write!(formatter, "{}", self.address),
            Some(prefix_length) => write!(formatter, "{}/{prefix_length}", self.address),
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    const CLIENT_PREFIX: u8 = 64;
    const NETWORK_PREFIX: u8 = 48;

    fn client(address: &str) -> ClientKey {
        ClientKey::of_client(address.parse().unwrap(), CLIENT_PREFIX)
    }

    #[test]
    fn ipv6_addresses_in_one_slash_64_share_a_client_key() {
        assert_eq!(
            client("2001:db8:1:2:aaaa:bbbb:cccc:dddd"),
            client("2001:db8:1:2::1")
        );
    }

    #[test]
    fn ipv6_addresses_in_different_slash_64_do_not() {
        assert_ne!(client("2001:db8:1:2::1"), client("2001:db8:1:3::1"));
    }

    #[test]
    fn ipv4_mapped_ipv6_keys_as_its_ipv4_address() {
        assert_eq!(client("::ffff:203.0.113.9"), client("203.0.113.9"));
    }

    #[test]
    fn distinct_ipv4_addresses_keep_distinct_keys() {
        assert_ne!(client("203.0.113.9"), client("203.0.113.10"));
    }

    #[test]
    fn the_client_prefix_length_decides_which_addresses_share_a_key() {
        let key = |address: &str, prefix| ClientKey::of_client(address.parse().unwrap(), prefix);
        assert_eq!(
            key("2001:db8:1:2:aaaa::1", 56),
            key("2001:db8:1:2:bbbb::9", 56)
        );
        assert_ne!(key("2001:db8:1:200::1", 56), key("2001:db8:1:300::1", 56));
        assert_ne!(key("2001:db8::1", 128), key("2001:db8::2", 128));
    }

    #[test]
    fn ipv6_clients_in_distinct_slash_64_of_one_slash_48_share_the_network_key() {
        let network = |address: &str| {
            ClientKey::of_ipv6_network(address.parse().unwrap(), NETWORK_PREFIX).unwrap()
        };
        assert_eq!(network("2001:db8:1:1::1"), network("2001:db8:1:ffff::1"));
        assert_ne!(network("2001:db8:1::1"), network("2001:db8:2::1"));
    }

    #[test]
    fn ipv4_clients_have_no_ipv6_network_key() {
        assert_eq!(
            ClientKey::of_ipv6_network("203.0.113.9".parse().unwrap(), NETWORK_PREFIX),
            None
        );
    }

    #[test]
    fn ipv4_mapped_clients_share_no_network_tier_key() {
        // Taken from the raw address, every IPv4-mapped client would land in the network `::/48`.
        assert_eq!(
            ClientKey::of_ipv6_network("::ffff:203.0.113.9".parse().unwrap(), NETWORK_PREFIX),
            None
        );
    }

    #[test]
    fn display_shows_the_ipv4_address_or_the_ipv6_network_with_its_prefix() {
        assert_eq!(client("203.0.113.9").to_string(), "203.0.113.9");
        assert_eq!(
            client("2001:db8:1:2:aaaa:bbbb:cccc:dddd").to_string(),
            "2001:db8:1:2::/64"
        );
        assert_eq!(
            ClientKey::of_ipv6_network("2001:db8:1:2::1".parse().unwrap(), NETWORK_PREFIX)
                .unwrap()
                .to_string(),
            "2001:db8:1::/48"
        );
    }
}
