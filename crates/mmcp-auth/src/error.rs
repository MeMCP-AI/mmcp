//! Error type for the auth layer.

use thiserror::Error;

use mmcp_db::DbError;

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

    /// Password is truly empty: zero bytes submitted. Distinct from
    /// [`AuthError::PasswordTooShort`] because a genuinely blank
    /// password carries no meaningful "actual length" a caller
    /// should read programmatically: there is nothing to report.
    #[error("password must not be empty")]
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

    /// A database lookup failed while checking whether a candidate
    /// handle is already taken during OAuth handle provisioning.
    /// Distinct from [`AuthError::Claims`]: a lookup failure is a
    /// database-layer failure, not a claims serialization or
    /// deserialization failure, and this variant preserves the
    /// underlying [`DbError`] as a walkable source instead of
    /// collapsing it to a bare string.
    #[error("failed to look up handle '{handle}'")]
    UserLookup {
        handle: String,
        #[source]
        source: DbError,
    },

    /// A first-time OAuth login attempted to auto-provision a new account.
    /// [`crate::backend::MmcpAuthBackend`] was constructed with self-registration disabled.
    /// Mirrors `POST /auth/register`'s `allow_self_registration` gate.
    /// This applies to the OAuth JIT-provisioning branch of `AuthnBackend::authenticate`.
    /// An existing linked account still authenticates normally.
    /// Only the auto-create-on-first-login path is rejected.
    #[error("self-registration is disabled; cannot auto-provision a new OAuth account")]
    SelfRegistrationDisabled,
}
