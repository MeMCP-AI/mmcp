//! [`ConfigKeyParseError`], a spelling that is not a configuration key.

use thiserror::Error;

/// The text is not the wire spelling of any [`ConfigKey`](super::ConfigKey).
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("unknown configuration key {input}")]
pub struct ConfigKeyParseError {
    /// The text that failed to parse.
    pub input: String,
}
