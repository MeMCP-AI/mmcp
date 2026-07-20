//! Axum router assembly.

use axum::Router;
use axum_login::AuthManagerLayerBuilder;
use tower_http::trace::TraceLayer;
use tower_sessions::MemoryStore;
use tower_sessions::SessionManagerLayer;

use crate::routes;
use crate::state::ServerState;

/// Build the top-level HTTP router with every route and middleware
/// layer attached.
pub fn build_router(state: ServerState) -> Router {
    // Session store: in-memory for now. A production deployment
    // should switch to a persistent store (Redis, SQLite, etc.)
    // to survive server restarts.
    let session_store = MemoryStore::default();
    let session_layer = SessionManagerLayer::new(session_store);

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
