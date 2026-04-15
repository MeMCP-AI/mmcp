//! Error type for the auth layer.

use thiserror::Error;

/// Failures returned by [`password`](crate::password) and
/// [`token`](crate::token).
#[derive(Debug, Error)]
pub enum AuthError {
    /// Argon2 hashing or verification failure.
    #[error("password error: {0}")]
    Password(String),

    /// Password hash did not match the supplied password.
    #[error("invalid credentials")]
    InvalidCredentials,

    /// Token issuance or verification failure.
    #[error("token error: {0}")]
    Token(String),

    /// Token is structurally valid but expired or not yet valid.
    #[error("token expired")]
    Expired,

    /// Serialization or deserialization of claims failed.
    #[error("claims error: {0}")]
    Claims(String),
}
