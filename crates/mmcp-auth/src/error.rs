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

    /// Password is empty, or trims to an empty string
    /// (whitespace-only). Distinct from
    /// [`AuthError::PasswordTooShort`] because a blank password
    /// carries no meaningful "actual length" a caller should read
    /// programmatically: reporting `actual: 0` for an 8-byte
    /// whitespace-only input would be false.
    #[error("password must not be empty or whitespace-only")]
    PasswordBlank,

    /// Password is shorter than the minimum accepted length, in
    /// bytes.
    #[error("password must be at least {min} bytes, got {actual}")]
    PasswordTooShort { min: usize, actual: usize },

    /// Password exceeds the maximum accepted length, in bytes.
    #[error("password must be at most {max} bytes, got {actual}")]
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

    /// Every OAuth-provisioned handle candidate, from the base
    /// `{provider}_{provider_user_id}` identifier through every
    /// numeric-suffix retry, collided with an existing user. Distinct
    /// from [`AuthError::Claims`]: this is a specific, actionable
    /// exhaustion condition, not an opaque downstream failure.
    #[error(
        "could not allocate a free handle for OAuth provider '{provider}' after {attempts} attempts"
    )]
    HandleAllocationExhausted { provider: String, attempts: u32 },
}
