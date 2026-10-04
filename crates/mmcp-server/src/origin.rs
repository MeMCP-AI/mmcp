//! Properties of the server's configured public origin.

/// Whether `origin` names an HTTPS endpoint.
///
/// Decides the `Secure` attribute of every cookie the server sets,
/// the session cookie ([`crate::app::build_router`]) and the OAuth flow cookie alike:
/// a browser refuses to store a `Secure` cookie received over plain
/// HTTP, so tying it to the deployment's actual scheme keeps a
/// loopback/HTTP dev origin working while still hardening a real
/// HTTPS deployment, instead of one fixed choice that breaks either.
pub(crate) fn origin_uses_https(origin: &str) -> bool {
    origin.starts_with("https://")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origin_uses_https_matches_only_the_https_scheme() {
        assert!(origin_uses_https("https://mmcp.example.com"));
        assert!(!origin_uses_https("http://localhost:8787"));
        assert!(!origin_uses_https("http://mmcp.example.com"));
    }
}
