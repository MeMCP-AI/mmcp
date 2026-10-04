//! [`TierScope`], which clients one budget of the limiter counts.

use std::fmt;
use std::net::IpAddr;

use super::client_key::ClientKey;

/// The clients one tier counts, and how it identifies each of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TierScope {
    /// Every client: an IPv4 address, or an IPv6 network of `ipv6_prefix_length` bits.
    Client {
        /// Leading bits of an IPv6 address that identify one client.
        ipv6_prefix_length: u8,
    },
    /// The IPv6 clients of one network of `prefix_length` bits, counted together.
    /// An IPv4 client is never counted by this tier.
    Ipv6Network {
        /// Leading bits of an IPv6 address that identify one network.
        prefix_length: u8,
    },
}

impl TierScope {
    /// The key under which this tier counts the client at `address`, `None` when the tier does not count it.
    #[must_use]
    pub fn key_of(self, address: IpAddr) -> Option<ClientKey> {
        match self {
            Self::Client { ipv6_prefix_length } => {
                Some(ClientKey::of_client(address, ipv6_prefix_length))
            }
            Self::Ipv6Network { prefix_length } => {
                ClientKey::of_ipv6_network(address, prefix_length)
            }
        }
    }
}

impl fmt::Display for TierScope {
    /// The scope in words, for the logs.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Client { ipv6_prefix_length } => {
                write!(
                    formatter,
                    "client (IPv4 address, IPv6 /{ipv6_prefix_length})"
                )
            }
            Self::Ipv6Network { prefix_length } => {
                write!(formatter, "IPv6 /{prefix_length} network")
            }
        }
    }
}
