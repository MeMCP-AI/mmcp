//! Unified `StoreError` type for the `mmcp-store` public surface.
//!
//! Every consumer-facing function in this crate, groups, memory, sync, diagnostics,
//! returns `Result<T, StoreError>`.
//! Consumers map this enum into their own outward shapes:
//! the CLI maps to `anyhow::Error` via `#[from]`,
//! the MCP tools map to `McpError::invalid_params` with a structured `code` payload,
//! and third-party callers match on variants directly.
//!
//! The enum is additive: new variants land when a module needs a code the current set doesn't cover.
//! Existing `#[from]` conversions keep `?` ergonomic at call sites without requiring explicit `.map_err` plumbing.

use thiserror::Error;

/// Failures returned by the mmcp-store surface.
#[derive(Debug, Error)]
pub enum StoreError {
    /// I/O failure while reading or writing a state file.
    #[error("store I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// TOML parse failure while loading a state file.
    #[error("store TOML parse error: {0}")]
    TomlParse(#[from] toml::de::Error),

    /// TOML serialize failure while writing a state file.
    #[error("store TOML serialize error: {0}")]
    TomlSerialize(#[from] toml::ser::Error),

    /// Git backend failure while opening a repository, reading a
    /// manifest, or committing a write.
    #[error("git backend error: {0}")]
    Git(#[from] mmcp_git::GitError),

    /// A group repository's manifest did not parse.
    ///
    /// Reserved for flows that must treat a broken manifest as
    /// fatal rather than logging-and-skipping; the group-index
    /// scanner still skips these to keep an unrelated broken repo
    /// from tanking the whole mirror.
    #[error("manifest error: {0}")]
    Manifest(#[from] mmcp_core::manifest::ManifestError),

    /// Transcript signing / compaction detection failure surfaced
    /// by `mmcp-session::compute_signature`. Used by the session
    /// store's `check_transcript` path.
    #[error("transcript signature error: {0}")]
    Transcript(#[from] mmcp_session::SessionError),
}

#[cfg(test)]
mod tests {
    use std::error::Error as _;

    use super::*;

    #[test]
    fn io_variant_carries_the_real_source() {
        let source = std::io::Error::new(std::io::ErrorKind::NotFound, "missing.toml");
        let err: StoreError = source.into();

        assert!(matches!(err, StoreError::Io(_)));
        let chained = err
            .source()
            .and_then(|s| s.downcast_ref::<std::io::Error>())
            .expect("io source must be preserved");
        assert_eq!(chained.kind(), std::io::ErrorKind::NotFound);
    }

    #[test]
    fn toml_parse_variant_carries_the_real_source() {
        let parse_err = toml::from_str::<toml::Value>("not = [valid").unwrap_err();
        let err: StoreError = parse_err.into();

        assert!(matches!(err, StoreError::TomlParse(_)));
        assert!(
            err.source()
                .and_then(|s| s.downcast_ref::<toml::de::Error>())
                .is_some()
        );
    }

    #[test]
    fn toml_serialize_variant_carries_the_real_source() {
        // A TOML document's top level must be a table; serializing a
        // bare scalar is the simplest value the `toml` crate's
        // serializer genuinely rejects.
        let ser_err = toml::to_string(&"not a table").unwrap_err();
        let err: StoreError = ser_err.into();

        assert!(matches!(err, StoreError::TomlSerialize(_)));
        assert!(
            err.source()
                .and_then(|s| s.downcast_ref::<toml::ser::Error>())
                .is_some()
        );
    }
}
