//! Properties of the server's configured public origin.

/// Whether `origin` names an HTTPS endpoint.
///
/// Decides the `Secure` attribute of every cookie the server sets.
/// That covers the session cookie ([`crate::app::build_router`]) and the OAuth flow cookie.
/// A browser refuses to store a `Secure` cookie received over plain HTTP.
/// Tying the attribute to the deployment's actual scheme keeps a loopback HTTP dev origin working.
/// It also keeps a real HTTPS deployment hardened, instead of one fixed choice that breaks either.
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
