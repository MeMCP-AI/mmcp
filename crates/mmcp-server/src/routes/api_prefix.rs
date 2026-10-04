//! Mounting of an API route group under its path prefix.

use axum::{Router, http::StatusCode, routing::any};

use crate::state::ServerState;

/// Bare `404` for a request under an API prefix that matches no route.
async fn api_not_found() -> StatusCode {
    StatusCode::NOT_FOUND
}

/// Mounts `routes` under `prefix` on `app`, claiming every path under `prefix`.
///
/// A request under `prefix` that matches no route answers a bare 404 instead of
/// reaching the fallback of `app`, so the web UI never answers for an API path.
/// A wrong method on a registered route still answers 405 from its own route.
pub fn mount_under_prefix(
    app: Router<ServerState>,
    prefix: &str,
    routes: Router<ServerState>,
) -> Router<ServerState> {
    // Nesting claims `prefix` and `prefix/<segment>...`; `prefix/` needs its own route.
    app.nest(prefix, routes.fallback(api_not_found))
        .route(&format!("{prefix}/"), any(api_not_found))
}
