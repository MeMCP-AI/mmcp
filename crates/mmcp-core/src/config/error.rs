//! Errors returned by the configuration loader and renderer.

use thiserror::Error;

/// Failure modes when reading or writing `.mmcp/config.toml`.
#[derive(Debug, Error)]
pub enum ConfigError {
    /// The TOML text could not be parsed.
    #[error("invalid TOML in mmcp config: {0}")]
    Parse(#[from] toml::de::Error),

    /// The parsed structure could not be serialized back to TOML.
    #[error("failed to render mmcp config: {0}")]
    Render(#[from] toml::ser::Error),
}
