//! Axum router assembly.

use axum::Router;
use axum::http::{HeaderValue, Method, header};
use axum_login::AuthManagerLayerBuilder;
use tower_http::cors::{AllowOrigin, CorsLayer};
use tower_http::trace::TraceLayer;
use tower_sessions::cookie::SameSite;
use tower_sessions::{Expiry, SessionManagerLayer};

use crate::defaults::SESSION_INACTIVITY_EXPIRY;
use crate::origin::origin_uses_https;
use crate::routes;
use crate::routes::api_prefix::mount_under_prefix;
use crate::state::ServerState;

/// Build the top-level HTTP router with every route and middleware
/// layer attached.
pub fn build_router(state: ServerState) -> Router {
    // tower-sessions defaults `same_site` to `Strict`.
    // A `Strict` cookie is withheld on the cross-site GET the OAuth redirect performs to the callback route.
    // The callback handler would then not see the caller's existing session.
    // `Lax` is the least permissive tier that survives the redirect while blocking cross-site POST/fetch/XHR CSRF.
    //
    // tower-sessions also defaults `secure` to `true` unconditionally.
    // See `origin_uses_https`'s doc comment for why `secure` is tied to the configured origin instead.
    let session_layer = SessionManagerLayer::new(state.session_store.clone())
        .with_same_site(SameSite::Lax)
        .with_secure(origin_uses_https(&state.origin))
        .with_expiry(Expiry::OnInactivity(SESSION_INACTIVITY_EXPIRY));

    let auth_layer =
        AuthManagerLayerBuilder::new(state.auth_backend.clone(), session_layer).build();

    // Each API prefix answers its own unmatched paths with 404,
    // so only a path outside every prefix reaches the web UI fallback.
    let api = Router::new().merge(routes::health::router());
    let api = mount_under_prefix(api, routes::mcp::PREFIX, routes::mcp::router());
    let api = mount_under_prefix(api, routes::auth::PREFIX, routes::auth::router());
    let api = mount_under_prefix(api, routes::sync::PREFIX, routes::sync::router());
    let api = mount_under_prefix(api, routes::git_http::PREFIX, routes::git_http::router());
    let routed = match state.webui_dir.as_deref().and_then(routes::webui::service) {
        Some(webui) => api.fallback_service(webui),
        None => api,
    };
    let cors_layer = cors_layer(&state.cors_allowed_origins);

    let router = routed
        .with_state(state)
        .layer(auth_layer)
        .layer(TraceLayer::new_for_http());
    match cors_layer {
        Some(cors_layer) => router.layer(cors_layer),
        None => router,
    }
}

/// CORS layer admitting exactly `allowed_origins`; `None` for an empty list,
/// which leaves the server answering same-origin requests only.
///
/// A cross-origin web UI authenticates with the bearer token, so credentials
/// (the session cookie) are never allowed.
fn cors_layer(allowed_origins: &[HeaderValue]) -> Option<CorsLayer> {
    if allowed_origins.is_empty() {
        return None;
    }
    Some(
        CorsLayer::new()
            .allow_origin(AllowOrigin::list(allowed_origins.iter().cloned()))
            .allow_methods([
                Method::GET,
                Method::POST,
                Method::PATCH,
                Method::DELETE,
                Method::OPTIONS,
            ])
            .allow_headers([header::AUTHORIZATION, header::CONTENT_TYPE, header::ACCEPT]),
    )
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    /// Falsification for the bound this constant is meant to enforce.
    /// Too short would fail a real OAuth, passkey or password login round trip.
    /// Too long would keep an abandoned anonymous session beyond the flow it exists to bound.
    /// See `crate::routes::auth::passkey_login_start` for that session.
    #[test]
    fn session_inactivity_expiry_is_a_short_minutes_scale_window() {
        assert!(SESSION_INACTIVITY_EXPIRY >= time::Duration::minutes(1));
        assert!(SESSION_INACTIVITY_EXPIRY <= time::Duration::minutes(30));
    }
}
