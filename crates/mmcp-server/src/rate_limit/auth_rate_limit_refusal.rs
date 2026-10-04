//! [`AuthRateLimitRefusal`], why the limiter turned a request away.

use axum::http::{StatusCode, header::RETRY_AFTER};
use axum::response::{IntoResponse, Response};

use super::client_key::ClientKey;
use crate::routes::response::GENERIC_INTERNAL_ERROR_MESSAGE;

/// A request the limiter did not let through.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthRateLimitRefusal {
    /// The client spent its budget.
    TooManyRequests {
        /// The client whose budget is spent, for the server log only.
        client: ClientKey,
        /// Seconds until the client may retry, rounded up and never below one.
        retry_after_seconds: u64,
    },
    /// The request carries no peer address, so no client can be identified.
    /// That is a wiring defect: the server was not served through [`crate::serve::serve_router`].
    PeerAddressMissing,
}

impl IntoResponse for AuthRateLimitRefusal {
    /// 429 with `Retry-After` for a spent budget, the generic 500 for a missing peer address.
    /// A request that cannot be attributed to a client fails closed and is never counted under a shared key.
    fn into_response(self) -> Response {
        match self {
            Self::TooManyRequests {
                retry_after_seconds,
                ..
            } => (
                StatusCode::TOO_MANY_REQUESTS,
                [(RETRY_AFTER, retry_after_seconds.to_string())],
                format!("too many requests; retry after {retry_after_seconds} seconds"),
            )
                .into_response(),
            Self::PeerAddressMissing => {
                tracing::error!(
                    "auth rate limit cannot identify the client because the server runs without client addresses"
                );
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    GENERIC_INTERNAL_ERROR_MESSAGE,
                )
                    .into_response()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use axum::body::to_bytes;

    use super::*;

    const RETRY_AFTER_SECONDS: u64 = 7;
    /// Bound on the response body a test reads.
    const BODY_LIMIT_BYTES: usize = 1024;

    async fn body_text(response: Response) -> String {
        let bytes = to_bytes(response.into_body(), BODY_LIMIT_BYTES)
            .await
            .unwrap();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    #[tokio::test]
    async fn too_many_requests_answers_429_with_retry_after_and_the_plain_text_body() {
        let response = AuthRateLimitRefusal::TooManyRequests {
            client: ClientKey::from_client_address("203.0.113.9".parse().unwrap()),
            retry_after_seconds: RETRY_AFTER_SECONDS,
        }
        .into_response();

        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(
            response.headers().get(RETRY_AFTER).unwrap(),
            &RETRY_AFTER_SECONDS.to_string()
        );
        assert_eq!(
            body_text(response).await,
            "too many requests; retry after 7 seconds"
        );
    }

    #[tokio::test]
    async fn a_missing_peer_address_answers_the_generic_500_without_detail() {
        let response = AuthRateLimitRefusal::PeerAddressMissing.into_response();

        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert!(response.headers().get(RETRY_AFTER).is_none());
        assert_eq!(body_text(response).await, GENERIC_INTERNAL_ERROR_MESSAGE);
    }
}
