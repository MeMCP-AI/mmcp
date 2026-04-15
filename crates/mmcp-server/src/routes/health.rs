//! Health check endpoint.

use axum::{Json, Router, routing::get};
use serde::Serialize;

use crate::state::ServerState;

#[derive(Serialize)]
struct HealthResponse {
    status: &'static str,
    name: &'static str,
    version: &'static str,
}

pub fn router() -> Router<ServerState> {
    Router::new().route("/health", get(health))
}

async fn health() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok",
        name: env!("CARGO_PKG_NAME"),
        version: env!("CARGO_PKG_VERSION"),
    })
}
