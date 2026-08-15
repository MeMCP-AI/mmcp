//! Axum router assembly.

use axum::Router;
use axum_login::AuthManagerLayerBuilder;
use tower_http::trace::TraceLayer;
use tower_sessions::cookie::SameSite;
use tower_sessions::{Expiry, MemoryStore, SessionManagerLayer};

use crate::defaults::SESSION_INACTIVITY_EXPIRY;
use crate::routes;
use crate::state::ServerState;

/// Whether `origin` names an HTTPS endpoint.
///
/// Decides the session cookie's `Secure` attribute in [`build_router`]:
/// a browser refuses to store a `Secure` cookie received over plain
/// HTTP, so tying it to the deployment's actual scheme keeps a
/// loopback/HTTP dev origin working while still hardening a real
/// HTTPS deployment, instead of one fixed choice that breaks either.
fn origin_uses_https(origin: &str) -> bool {
    origin.starts_with("https://")
}

/// Build the top-level HTTP router with every route and middleware
/// layer attached.
pub fn build_router(state: ServerState) -> Router {
    // Session store: in-memory for now. A production deployment
    // should switch to a persistent store (Redis, SQLite, etc.)
    // to survive server restarts.
    let session_store = MemoryStore::default();

    // tower-sessions defaults `same_site` to `Strict`.
    // Browsers withhold a `Strict` cookie on the cross-site top-level
    // GET the OAuth provider's redirect performs back to this
    // server's callback route, so the CSRF state stored at authorize
    // time was never observed at callback and every real OAuth login
    // failed. `Lax` is the least permissive tier that still survives
    // that navigation while still blocking cross-site POST/fetch/XHR
    // CSRF.
    //
    // tower-sessions also defaults `secure` to `true` unconditionally;
    // see `origin_uses_https`'s doc comment for why that is tied to
    // the configured origin instead.
    let session_layer = SessionManagerLayer::new(session_store)
        .with_same_site(SameSite::Lax)
        .with_secure(origin_uses_https(&state.origin))
        .with_expiry(Expiry::OnInactivity(SESSION_INACTIVITY_EXPIRY));

    let auth_layer =
        AuthManagerLayerBuilder::new(state.auth_backend.clone(), session_layer).build();

    Router::new()
        .merge(routes::health::router())
        .merge(routes::mcp::router())
        .merge(routes::auth::router())
        .merge(routes::sync::router())
        .merge(routes::git_http::router())
        .with_state(state)
        .layer(auth_layer)
        .layer(TraceLayer::new_for_http())
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

    /// Falsification for the bound this constant is meant to enforce:
    /// too short would fail a real OAuth or password login round
    /// trip, too long would let an abandoned anonymous session (see
    /// `crate::routes::auth::oauth_authorize`) sit in the in-memory
    /// store far longer than the flow it exists to bound.
    #[test]
    fn session_inactivity_expiry_is_a_short_minutes_scale_window() {
        assert!(SESSION_INACTIVITY_EXPIRY >= time::Duration::minutes(1));
        assert!(SESSION_INACTIVITY_EXPIRY <= time::Duration::minutes(30));
    }
}
