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

    /// `MMCP_TOKEN_KEY_HEX` was explicitly set but is not exactly 64
    /// characters long (`got` is the raw input's character count,
    /// checked BEFORE hex decoding, distinct from
    /// [`ConfigError::TokenKeyHexInvalidCharacter`] below). Fails
    /// construction instead of silently substituting a freshly
    /// generated random key: an operator who set a stable key
    /// deliberately must see the rejection, not a server that
    /// silently signs sessions with a different key on every restart.
    /// The rejected value itself is never carried on this variant: it
    /// is the (rejected) key material.
    #[error(
        "MMCP_TOKEN_KEY_HEX was rejected: expected 64 hex characters \
         encoding 32 bytes, got {got}"
    )]
    TokenKeyHexWrongLength { got: usize },

    /// `MMCP_TOKEN_KEY_HEX` was explicitly set, is exactly 64
    /// characters long, but contains at least one byte outside the
    /// hex alphabet. Deliberately fieldless and NOT source-chained to
    /// `hex::FromHexError`: that type's `InvalidHexCharacter` variant
    /// embeds the offending character, which is drawn from what is
    /// meant to be key material and must never surface in an error or
    /// log line. Fails construction for the same reason
    /// [`ConfigError::TokenKeyHexWrongLength`] does.
    #[error("MMCP_TOKEN_KEY_HEX was rejected: contains a non-hex character")]
    TokenKeyHexInvalidCharacter,

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
