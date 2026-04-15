//! Axum router assembly.

use axum::Router;
use tower_http::trace::TraceLayer;

use crate::routes;
use crate::state::ServerState;

/// Build the top-level HTTP router with every route and middleware
/// layer attached.
pub fn build_router(state: ServerState) -> Router {
    Router::new()
        .merge(routes::health::router())
        .merge(routes::mcp::router())
        .merge(routes::auth::router())
        .with_state(state)
        .layer(TraceLayer::new_for_http())
}
