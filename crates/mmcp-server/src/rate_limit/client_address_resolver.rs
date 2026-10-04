//! [`ClientAddressResolver`], the client address of a request behind trusted reverse proxies.

use std::net::{IpAddr, SocketAddr};

use axum::http::HeaderMap;
use axum_client_addr::{ChainHeader, ClientIpConfig, TrustedProxyRule};

use crate::config::TrustedProxies;

/// Resolves the client address of a request.
///
/// A request whose TCP peer lies in a trusted range is attributed to the first address of its `X-Forwarded-For` header, read from the right, that is not itself a trusted proxy.
/// Any other request is attributed to its TCP peer, and its forwarded headers are ignored.
/// `X-Forwarded-For` is the only header read: `Forwarded` and `X-Real-IP` never are.
#[derive(Debug, Clone)]
pub struct ClientAddressResolver {
    config: ClientIpConfig,
}

impl ClientAddressResolver {
    /// A resolver trusting exactly `trusted_proxies`.
    ///
    /// An empty list trusts no proxy.
    /// A list the resolver cannot combine trusts no proxy either, with a warning, which narrows trust and never widens it.
    #[must_use]
    pub fn new(trusted_proxies: &TrustedProxies) -> Self {
        let trust_no_proxy = || ClientIpConfig::builder().trust_no_proxy();
        if trusted_proxies.is_empty() {
            return Self {
                config: trust_no_proxy(),
            };
        }
        let config = ClientIpConfig::builder()
            .trusted_proxies()
            .chain_header_order([ChainHeader::x_forwarded_for()])
            .proxies(
                trusted_proxies
                    .ranges()
                    .iter()
                    .copied()
                    .map(TrustedProxyRule::new),
            )
            .build()
            .unwrap_or_else(|error| {
                tracing::warn!(
                    %error,
                    "trusted proxy ranges could not be combined; trusting no proxy"
                );
                trust_no_proxy()
            });
        Self { config }
    }

    /// The client address of a request with `headers` that arrived from `peer`.
    #[must_use]
    pub fn resolve(&self, headers: &HeaderMap, peer: SocketAddr) -> IpAddr {
        self.config.resolve_client_ip(headers, peer.ip()).ip()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use axum::http::HeaderValue;

    use super::*;

    const TRUSTED_PROXY: &str = "10.0.0.2";
    const OTHER_TRUSTED_PROXY: &str = "10.0.0.3";
    const UNTRUSTED_PEER: &str = "192.0.2.50";
    const CLIENT: &str = "203.0.113.10";
    const SPOOFED_CLIENT: &str = "198.51.100.99";
    /// Source port of every simulated connection; the resolver ignores it.
    const PEER_PORT: u16 = 40_000;

    fn resolver(trusted: &str) -> ClientAddressResolver {
        ClientAddressResolver::new(&TrustedProxies::parse(trusted))
    }

    fn headers(name: &'static str, value: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(name, HeaderValue::from_str(value).unwrap());
        headers
    }

    fn peer(address: &str) -> SocketAddr {
        SocketAddr::new(address.parse().unwrap(), PEER_PORT)
    }

    fn resolved(resolver: &ClientAddressResolver, headers: &HeaderMap, address: &str) -> String {
        resolver.resolve(headers, peer(address)).to_string()
    }

    #[test]
    fn forwarded_header_from_an_untrusted_peer_is_ignored() {
        let resolver = resolver("10.0.0.0/24");
        let headers = headers("x-forwarded-for", SPOOFED_CLIENT);
        assert_eq!(
            resolved(&resolver, &headers, UNTRUSTED_PEER),
            UNTRUSTED_PEER
        );
    }

    #[test]
    fn no_configured_proxy_means_every_peer_is_its_own_client() {
        let resolver = resolver("");
        let headers = headers("x-forwarded-for", SPOOFED_CLIENT);
        assert_eq!(resolved(&resolver, &headers, TRUSTED_PROXY), TRUSTED_PROXY);
    }

    #[test]
    fn rightmost_untrusted_forwarded_address_is_the_client_behind_trusted_proxies() {
        let resolver = resolver("10.0.0.0/24");
        // The client wrote the spoofed address, the first proxy appended the client, the second appended the first proxy.
        let headers = headers(
            "x-forwarded-for",
            &format!("{SPOOFED_CLIENT}, {CLIENT}, {OTHER_TRUSTED_PROXY}"),
        );
        assert_eq!(resolved(&resolver, &headers, TRUSTED_PROXY), CLIENT);
    }

    #[test]
    fn forwarded_chain_of_only_trusted_or_malformed_entries_falls_back_to_the_peer() {
        let resolver = resolver("10.0.0.0/24");
        let only_trusted = headers("x-forwarded-for", OTHER_TRUSTED_PROXY);
        assert_eq!(
            resolved(&resolver, &only_trusted, TRUSTED_PROXY),
            TRUSTED_PROXY
        );
        let malformed = headers("x-forwarded-for", "not-an-address");
        assert_eq!(
            resolved(&resolver, &malformed, TRUSTED_PROXY),
            TRUSTED_PROXY
        );
    }

    #[test]
    fn a_trusted_peer_without_a_forwarded_header_is_its_own_client() {
        let resolver = resolver("10.0.0.0/24");
        assert_eq!(
            resolved(&resolver, &HeaderMap::new(), TRUSTED_PROXY),
            TRUSTED_PROXY
        );
    }

    #[test]
    fn forwarded_header_behind_a_trusted_proxy_is_ignored() {
        let resolver = resolver("10.0.0.0/24");
        // A proxy that forwards a client-written `Forwarded` header must not let the client choose its key.
        let headers = headers("forwarded", &format!("for={SPOOFED_CLIENT}"));
        assert_eq!(resolved(&resolver, &headers, TRUSTED_PROXY), TRUSTED_PROXY);
    }

    #[test]
    fn x_real_ip_behind_a_trusted_proxy_is_ignored() {
        let resolver = resolver("10.0.0.0/24");
        let headers = headers("x-real-ip", SPOOFED_CLIENT);
        assert_eq!(resolved(&resolver, &headers, TRUSTED_PROXY), TRUSTED_PROXY);
    }

    #[test]
    fn unreadable_forwarded_chain_from_a_trusted_peer_keys_the_peer() {
        let resolver = resolver("10.0.0.0/24");
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-forwarded-for",
            HeaderValue::from_bytes(b"203.0.113.10, \xff").unwrap(),
        );
        assert_eq!(resolved(&resolver, &headers, TRUSTED_PROXY), TRUSTED_PROXY);
    }

    #[test]
    fn several_forwarded_lines_are_read_as_one_chain_in_arrival_order() {
        let resolver = resolver("10.0.0.0/24");
        let mut headers = HeaderMap::new();
        headers.append("x-forwarded-for", HeaderValue::from_static(SPOOFED_CLIENT));
        headers.append("x-forwarded-for", HeaderValue::from_static(CLIENT));
        assert_eq!(resolved(&resolver, &headers, TRUSTED_PROXY), CLIENT);
    }

    #[test]
    fn an_ipv4_mapped_peer_matches_an_ipv4_trusted_range() {
        let resolver = resolver("10.0.0.0/24");
        let headers = headers("x-forwarded-for", CLIENT);
        assert_eq!(
            resolver
                .resolve(&headers, peer("::ffff:10.0.0.2"))
                .to_string(),
            CLIENT
        );
    }

    #[test]
    fn an_ipv6_client_behind_a_trusted_proxy_is_resolved_from_the_chain() {
        let resolver = resolver("10.0.0.0/24");
        let headers = headers("x-forwarded-for", "2001:db8::17");
        assert_eq!(resolved(&resolver, &headers, TRUSTED_PROXY), "2001:db8::17");
    }
}
