//! [`ConfigError`], the failure type for [`super::ServerConfig`] construction.

use thiserror::Error;

/// Failure modes of [`super::ServerConfig`] construction.
#[derive(Debug, Error)]
pub enum ConfigError {
    /// The OS CSPRNG could not be read while generating a random
    /// session-signing key (used when `MMCP_TOKEN_KEY_HEX` is unset
    /// or rejected). Genuinely reachable, not merely theoretical: a
    /// sandboxed or otherwise constrained environment can lack an
    /// entropy source. The operator's remedy is to set
    /// `MMCP_TOKEN_KEY_HEX` explicitly there.
    #[error(
        "OS CSPRNG unavailable while generating a random session-signing key; \
         set MMCP_TOKEN_KEY_HEX explicitly on this environment"
    )]
    RandomKeyUnavailable(#[source] getrandom::Error),

    /// `MMCP_BIND` was explicitly set but the value is not a valid
    /// socket address. Fails construction instead of silently
    /// substituting the compiled-in default: an operator who set a
    /// bind address deliberately must see the rejection, not a
    /// server that quietly listens somewhere else.
    #[error("MMCP_BIND={raw:?} is not a valid socket address: {source}")]
    InvalidBind {
        raw: String,
        #[source]
        source: std::net::AddrParseError,
    },

    /// `MMCP_TOKEN_KEY_HEX` was explicitly set but the value failed
    /// hex-key validation (wrong length, or characters outside the
    /// hex alphabet). Fails construction instead of silently
    /// substituting a freshly generated random key: an operator who
    /// set a stable key deliberately must see the rejection, not a
    /// server that silently signs sessions with a different key on
    /// every restart. The rejected value itself is never carried on
    /// this variant: it is the (rejected) key material.
    #[error("MMCP_TOKEN_KEY_HEX was rejected: must be 64 hex characters encoding 32 bytes")]
    InvalidTokenKeyHex,

    /// The effective minimum password length, resolved from the
    /// override/env/config-file/default cascade, exceeds the
    /// effective maximum password length resolved from the same
    /// cascade. Left unchecked, this would silently start a server on
    /// which every account registration fails password validation.
    #[error(
        "resolved minimum password length ({min}) exceeds the resolved maximum \
         password length ({max}); every account registration would fail \
         password validation"
    )]
    MinPasswordLengthExceedsMax { min: usize, max: usize },
}
