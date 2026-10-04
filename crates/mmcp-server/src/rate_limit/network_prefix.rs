//! The one place an IPv6 address is cut down to its leading bits.

use std::net::Ipv6Addr;

use crate::config::IPV6_ADDRESS_BITS;

/// The network of `address`: its first `prefix_length` bits, the rest zero.
///
/// Every tier that counts IPv6 clients by prefix calls this one function.
/// A length of 0 gives the unspecified address and a length of [`IPV6_ADDRESS_BITS`] or more keeps the address whole.
pub(super) fn ipv6_network(address: Ipv6Addr, prefix_length: u8) -> Ipv6Addr {
    let kept_bits = prefix_length.min(IPV6_ADDRESS_BITS);
    let dropped_bits = u32::from(IPV6_ADDRESS_BITS - kept_bits);
    let mask = u128::MAX.checked_shl(dropped_bits).unwrap_or(0);
    Ipv6Addr::from(u128::from(address) & mask)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    fn network(address: &str, prefix_length: u8) -> String {
        ipv6_network(address.parse().unwrap(), prefix_length).to_string()
    }

    #[test]
    fn the_leading_bits_are_kept_and_the_rest_is_zeroed() {
        let address = "2001:db8:1:2ff:aaaa:bbbb:cccc:dddd";
        assert_eq!(network(address, 64), "2001:db8:1:2ff::");
        assert_eq!(network(address, 56), "2001:db8:1:200::");
        assert_eq!(network(address, 48), "2001:db8:1::");
    }

    #[test]
    fn a_prefix_that_is_not_a_multiple_of_sixteen_cuts_inside_a_group() {
        assert_eq!(network("2001:db8:ffff::1", 36), "2001:db8:f000::");
    }

    #[test]
    fn the_extreme_lengths_are_total() {
        let address = "2001:db8::1";
        assert_eq!(network(address, 0), "::");
        assert_eq!(network(address, 128), address);
        assert_eq!(network(address, u8::MAX), address);
    }
}
