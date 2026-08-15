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
}
