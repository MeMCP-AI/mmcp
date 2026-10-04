//! [`TrustedProxies`], the reverse proxies whose forwarded client address the server believes.

use std::fmt;

use axum_client_addr::IpCidr;

use super::env_value::non_blank;

/// Separator of the entries of a trusted proxy list.
const ENTRY_SEPARATOR: char = ',';

/// IP addresses and CIDR ranges of the reverse proxies in front of the server.
///
/// A request whose TCP peer lies in one of these ranges is attributed to the client address its `X-Forwarded-For` header gives.
/// Any other request is attributed to its TCP peer.
/// The default is empty, which trusts no proxy.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TrustedProxies {
    ranges: Vec<IpCidr>,
}

impl TrustedProxies {
    /// Parse a comma-separated list of IP addresses and CIDR ranges.
    ///
    /// A blank list is empty.
    /// An entry that is neither an address nor a range is dropped with a warning, which narrows trust and never widens it.
    #[must_use]
    pub fn parse(raw: &str) -> Self {
        let Some(list) = non_blank(Some(raw)) else {
            return Self::default();
        };
        let ranges = list
            .split(ENTRY_SEPARATOR)
            .map(str::trim)
            .filter(|entry| !entry.is_empty())
            .filter_map(|entry| match entry.parse::<IpCidr>() {
                Ok(range) => Some(range),
                Err(_) => {
                    tracing::warn!(
                        entry,
                        "trusted proxy entry is not an IP address or CIDR range; ignoring it"
                    );
                    None
                }
            })
            .collect();
        Self { ranges }
    }

    /// The trusted ranges, in the order they were listed.
    #[must_use]
    pub fn ranges(&self) -> &[IpCidr] {
        &self.ranges
    }

    /// Whether no proxy is trusted.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.ranges.is_empty()
    }
}

impl fmt::Display for TrustedProxies {
    /// The ranges comma-separated, or `none` when no proxy is trusted.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.ranges.is_empty() {
            return formatter.write_str("none");
        }
        for (index, range) in self.ranges.iter().enumerate() {
            if index > 0 {
                formatter.write_str(", ")?;
            }
            write!(formatter, "{range}")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use crate::config::test_support::WarnCounter;

    fn range(text: &str) -> IpCidr {
        text.parse().unwrap()
    }

    #[test]
    fn blank_list_trusts_no_proxy() {
        assert!(TrustedProxies::parse("").is_empty());
        assert!(TrustedProxies::parse("  \n").is_empty());
        assert!(TrustedProxies::default().is_empty());
    }

    #[test]
    fn addresses_and_ranges_are_parsed_in_order_with_whitespace_ignored() {
        let proxies = TrustedProxies::parse(" 127.0.0.1 , 10.0.0.0/8,2001:db8::/32 ");
        assert_eq!(
            proxies.ranges(),
            [
                range("127.0.0.1"),
                range("10.0.0.0/8"),
                range("2001:db8::/32")
            ]
        );
    }

    #[test]
    fn a_bare_address_is_a_range_of_one_address() {
        let proxies = TrustedProxies::parse("192.0.2.7");
        assert_eq!(proxies.ranges(), [range("192.0.2.7/32")]);
    }

    #[test]
    fn trusted_proxies_skip_a_malformed_entry_with_a_warning() {
        let warnings = Arc::new(AtomicUsize::new(0));
        let proxies = tracing::subscriber::with_default(WarnCounter(warnings.clone()), || {
            TrustedProxies::parse("10.0.0.0/8,not-an-address,*,192.0.2.1")
        });
        assert_eq!(proxies.ranges(), [range("10.0.0.0/8"), range("192.0.2.1")]);
        assert_eq!(warnings.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn empty_entries_between_separators_are_skipped_without_a_warning() {
        let warnings = Arc::new(AtomicUsize::new(0));
        let proxies = tracing::subscriber::with_default(WarnCounter(warnings.clone()), || {
            TrustedProxies::parse(",10.0.0.0/8,, ,")
        });
        assert_eq!(proxies.ranges(), [range("10.0.0.0/8")]);
        assert_eq!(warnings.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn display_lists_the_ranges_or_none() {
        assert_eq!(TrustedProxies::default().to_string(), "none");
        assert_eq!(
            TrustedProxies::parse("10.0.0.0/8,192.0.2.1").to_string(),
            "10.0.0.0/8, 192.0.2.1"
        );
    }
}
