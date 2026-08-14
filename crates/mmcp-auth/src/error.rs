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

    /// Password is shorter than the minimum accepted length, or
    /// trims to an empty string (empty or whitespace-only).
    #[error("password must be at least {min} characters, got {actual}")]
    PasswordTooShort { min: usize, actual: usize },

    /// Password exceeds the maximum accepted length.
    #[error("password must be at most {max} characters, got {actual}")]
    PasswordTooLong { max: usize, actual: usize },

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
