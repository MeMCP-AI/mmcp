//! Axum router assembly.

use axum::Router;
use axum_login::AuthManagerLayerBuilder;
use tower_http::trace::TraceLayer;
use tower_sessions::cookie::SameSite;
use tower_sessions::{Expiry, SessionManagerLayer};

use crate::defaults::SESSION_INACTIVITY_EXPIRY;
use crate::origin::origin_uses_https;
use crate::routes;
use crate::state::ServerState;

/// Build the top-level HTTP router with every route and middleware
/// layer attached.
pub fn build_router(state: ServerState) -> Router {
    // tower-sessions defaults `same_site` to `Strict`.
    // A `Strict` cookie is withheld on the cross-site GET the OAuth redirect performs to the callback route,
    // so the callback handler would not see the caller's existing session.
    // `Lax` is the least permissive tier that survives the redirect while blocking cross-site POST/fetch/XHR CSRF.
    //
    // tower-sessions also defaults `secure` to `true` unconditionally;
    // see `origin_uses_https`'s doc comment for why that is tied to
    // the configured origin instead.
    let session_layer = SessionManagerLayer::new(state.session_store.clone())
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
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    /// Falsification for the bound this constant is meant to enforce:
    /// too short would fail a real OAuth, passkey or password login round
    /// trip, too long would let an abandoned anonymous session (see
    /// `crate::routes::auth::passkey_login_start`) sit in the session
    /// store far longer than the flow it exists to bound.
    #[test]
    fn session_inactivity_expiry_is_a_short_minutes_scale_window() {
        assert!(SESSION_INACTIVITY_EXPIRY >= time::Duration::minutes(1));
        assert!(SESSION_INACTIVITY_EXPIRY <= time::Duration::minutes(30));
    }
}
