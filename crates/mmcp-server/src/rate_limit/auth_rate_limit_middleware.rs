//! Axum middleware spending the rate limit budget of a request.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::{ConnectInfo, Request, State};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use super::auth_rate_limit_refusal::AuthRateLimitRefusal;
use super::auth_rate_limiter::AuthRateLimiter;

/// Spend one request of the caller's budget, then run the route, or refuse with the limiter's answer.
///
/// It runs before the route's own work, so a refused request costs one map operation and no password hash, database query or outbound request.
/// The peer address comes from the connection, which only [`crate::serve::serve_router`] provides.
/// A request without one fails closed.
pub async fn enforce_auth_rate_limit(
    State(rate_limiter): State<Arc<AuthRateLimiter>>,
    request: Request,
    next: Next,
) -> Response {
    let peer = request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ConnectInfo(peer)| *peer);
    match rate_limiter.check(request.headers(), peer) {
        Ok(()) => next.run(request).await,
        Err(refusal) => {
            if let AuthRateLimitRefusal::TooManyRequests {
                client,
                retry_after_seconds,
            } = &refusal
            {
                tracing::debug!(
                    %client,
                    route = request.uri().path(),
                    retry_after_seconds,
                    "auth request refused by the rate limit"
                );
            }
            refusal.into_response()
        }
    }
}
