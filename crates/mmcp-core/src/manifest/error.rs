//! Error type returned by the manifest parser.

use thiserror::Error;

/// Failures that can occur while reading or writing a
/// [`GroupManifest`](crate::manifest::GroupManifest).
#[derive(Debug, Error)]
pub enum ManifestError {
    /// The TOML text could not be parsed.
    #[error("invalid TOML in group manifest: {0}")]
    Parse(#[from] toml::de::Error),

    /// The parsed structure could not be serialized back to TOML.
    #[error("failed to render group manifest: {0}")]
    Render(#[from] toml::ser::Error),

    /// The manifest's `schema_version` is newer than the current
    /// mmcp build supports. The caller should surface this as a
    /// user-facing upgrade hint rather than silently discarding the
    /// manifest.
    #[error("manifest schema version {found} is newer than supported {supported}")]
    UnsupportedSchema { found: u32, supported: u32 },
}
