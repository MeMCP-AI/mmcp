//! Parsing of the CORS allowed-origins list.

use axum::http::HeaderValue;
use url::Url;

/// Outcome of [`parse_cors_origins`]: every entry lands in exactly one field.
#[derive(Debug, Default)]
pub(super) struct ParsedCorsOrigins {
    /// Canonical origins, in list order.
    pub accepted: Vec<HeaderValue>,
    /// Entries that are not an exact `http` or `https` origin, as written.
    pub rejected: Vec<String>,
}

/// Parse a comma-separated list of exact origins (`scheme://host[:port]`).
///
/// An entry is trimmed and compared in the form a browser sends in the
/// `Origin` header, so a default port or a trailing `/` is accepted and
/// normalized away.
/// Empty entries (a trailing comma, a blank list) are skipped, not rejected.
pub(super) fn parse_cors_origins(raw: &str) -> ParsedCorsOrigins {
    let mut parsed = ParsedCorsOrigins::default();
    for entry in raw.split(',').map(str::trim).filter(|e| !e.is_empty()) {
        match canonical_origin(entry) {
            Some(origin) => parsed.accepted.push(origin),
            None => parsed.rejected.push(entry.to_owned()),
        }
    }
    parsed
}

/// The `Origin` header value for `entry`, `None` unless `entry` is exactly an origin.
fn canonical_origin(entry: &str) -> Option<HeaderValue> {
    let url = Url::parse(entry).ok()?;
    let is_bare_origin = matches!(url.scheme(), "http" | "https")
        && url.has_host()
        && url.username().is_empty()
        && url.password().is_none()
        && url.path() == "/"
        && url.query().is_none()
        && url.fragment().is_none();
    if !is_bare_origin {
        return None;
    }
    HeaderValue::from_str(&url.origin().ascii_serialization()).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn accepted_strings(parsed: &ParsedCorsOrigins) -> Vec<&str> {
        parsed
            .accepted
            .iter()
            .map(|origin| origin.to_str().expect("origins are ASCII"))
            .collect()
    }

    #[test]
    fn exact_origins_are_accepted_in_list_order() {
        let parsed = parse_cors_origins("http://127.0.0.1:3000,https://mmcp.example.com");
        assert_eq!(
            accepted_strings(&parsed),
            ["http://127.0.0.1:3000", "https://mmcp.example.com"]
        );
        assert!(parsed.rejected.is_empty());
    }

    #[test]
    fn whitespace_trailing_slash_and_default_port_are_normalized() {
        let parsed = parse_cors_origins(" http://localhost:3000/ , https://a.example:443 ");
        assert_eq!(
            accepted_strings(&parsed),
            ["http://localhost:3000", "https://a.example"]
        );
    }

    #[test]
    fn blank_list_and_empty_entries_yield_nothing() {
        for raw in ["", "   ", ",", " , ,"] {
            let parsed = parse_cors_origins(raw);
            assert!(parsed.accepted.is_empty(), "accepted for {raw:?}");
            assert!(parsed.rejected.is_empty(), "rejected for {raw:?}");
        }
    }

    #[test]
    fn anything_but_a_bare_http_origin_is_rejected_as_written() {
        let entries = [
            "*",
            "localhost:3000",
            "ftp://example.com",
            "http://example.com/app",
            "http://example.com?x=1",
            "http://example.com/#frag",
            "http://user:pw@example.com",
            "not an origin",
        ];
        let parsed = parse_cors_origins(&entries.join(","));
        assert!(parsed.accepted.is_empty());
        assert_eq!(parsed.rejected, entries);
    }

    #[test]
    fn valid_and_invalid_entries_are_split_without_losing_either() {
        let parsed = parse_cors_origins("http://a.example,*,https://b.example");
        assert_eq!(
            accepted_strings(&parsed),
            ["http://a.example", "https://b.example"]
        );
        assert_eq!(parsed.rejected, ["*"]);
    }
}
