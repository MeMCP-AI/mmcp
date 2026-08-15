//! Bearer-token extractor for the authenticated `/sync/*` control plane.
//!
//! Verifies the same per-user PASETO v4 local token
//! [`routes::auth::login`](crate::routes::auth) issues via
//! [`mmcp_auth::TokenIssuer::issue`], returning the caller's real user
//! id from the token's `sub` claim.
//!
//! Distinct from the shared-secret `MMCP_PUSH_TOKEN` scheme
//! (`git_http::enforce_write`), which authorizes the raw git content
//! plane for a machine caller rather than identifying a real user.

use axum::extract::FromRequestParts;
use axum::http::StatusCode;
use axum::http::header::AUTHORIZATION;
use axum::http::request::Parts;
use axum::response::{IntoResponse, Response};
use jiff::Timestamp;
use mmcp_auth::AuthError;
use uuid::Uuid;

use crate::state::ServerState;

/// Challenge header value advertised on every bearer-auth rejection,
/// matching the scheme this extractor actually accepts.
const WWW_AUTHENTICATE_BEARER: &str = r#"Bearer realm="mmcp""#;

/// Caller-visible body for a bearer-auth rejection. Carries no detail
/// beyond "authenticate": the specific cause (missing header versus a
/// verification failure) is logged server-side only, per
/// `global-security-rules`'s error-message hygiene.
const REJECTION_MESSAGE: &str = "missing or invalid bearer token";

/// An authenticated caller of a bearer-guarded route, carrying the
/// user id verified from the session token's `sub` claim.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuthenticatedUser {
    pub user_id: Uuid,
}

/// Why a bearer-auth extraction failed.
///
/// Each cause is its own variant with its own log line even though
/// every variant renders the same generic 401 externally: the uniform
/// response avoids handing an unauthenticated caller a signal about
/// which part of its request was wrong.
#[derive(Debug)]
pub enum BearerAuthRejection {
    /// No `Authorization: Bearer <token>` header was present.
    MissingHeader,
    /// A header was present but the token failed verification
    /// (malformed, wrong key, or expired).
    Invalid(AuthError),
}

impl IntoResponse for BearerAuthRejection {
    fn into_response(self) -> Response {
        match &self {
            BearerAuthRejection::MissingHeader => {
                tracing::debug!("sync auth rejected: no bearer token presented");
            }
            BearerAuthRejection::Invalid(err) => {
                tracing::warn!(error = %err, "sync auth rejected: bearer token failed verification");
            }
        }
        (
            StatusCode::UNAUTHORIZED,
            [("WWW-Authenticate", WWW_AUTHENTICATE_BEARER)],
            REJECTION_MESSAGE,
        )
            .into_response()
    }
}

impl FromRequestParts<ServerState> for AuthenticatedUser {
    type Rejection = BearerAuthRejection;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &ServerState,
    ) -> Result<Self, Self::Rejection> {
        let token = parts
            .headers
            .get(AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Bearer "))
            .ok_or(BearerAuthRejection::MissingHeader)?;

        let now_secs = Timestamp::now().as_second();
        let claims = state
            .token_verifier
            .verify(token, now_secs)
            .map_err(BearerAuthRejection::Invalid)?;

        Ok(AuthenticatedUser {
            user_id: claims.sub,
        })
    }
}
