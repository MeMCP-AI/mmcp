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

use std::path::PathBuf;

use thiserror::Error;

mod file_operation;

pub use file_operation::FileOperation;

/// Failures returned by the mmcp-store surface.
#[derive(Debug, Error)]
pub enum StoreError {
    /// I/O failure while performing `operation` on `path`.
    #[error("failed to {operation} {path}: {source}", path = path.display())]
    Io {
        /// Path the filesystem operation targeted.
        path: PathBuf,
        /// Which filesystem action failed.
        operation: FileOperation,
        /// The underlying OS error.
        #[source]
        source: std::io::Error,
    },

    /// TOML parse failure while loading `path`.
    #[error("failed to parse TOML at {path}: {source}", path = path.display())]
    TomlParse {
        /// Path of the file whose contents failed to parse.
        path: PathBuf,
        /// The underlying parse error.
        #[source]
        source: toml::de::Error,
    },

    /// TOML serialize failure while rendering `path`.
    #[error("failed to serialize TOML for {path}: {source}", path = path.display())]
    TomlSerialize {
        /// Path the serialized document was destined for.
        path: PathBuf,
        /// The underlying serialize error.
        #[source]
        source: toml::ser::Error,
    },

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

    /// Transcript signing / compaction detection failure surfaced by `mmcp-session::compute_signature`.
    /// Used by the session store's `check_transcript` path.
    #[error("transcript signature error: {0}")]
    Transcript(#[from] mmcp_session::SessionError),

    /// A project-level `.mmcp.toml` failed to parse or render.
    /// `ProjectConfig` owns its own error type; this variant chains
    /// it rather than re-deriving a TOML parse/serialize shape here.
    #[error("project config error: {0}")]
    Config(#[from] mmcp_core::config::ConfigError),

    /// Sync client construction failed for `server_url`.
    #[error("failed to configure sync client for {server_url}: {source}")]
    Sync {
        /// The server URL the client was being configured for.
        server_url: String,
        /// The underlying sync-client error.
        #[source]
        source: mmcp_sync::SyncError,
    },

    /// Neither `MMCP_HOME`, `HOME`, nor `USERPROFILE` was set, so the
    /// mmcp home directory could not be resolved.
    #[error("cannot determine home directory: set MMCP_HOME, HOME, or USERPROFILE")]
    HomeDirUnresolved,
}

#[cfg(test)]
mod tests {
    use std::error::Error as _;
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn io_variant_carries_the_real_source_and_path() {
        let path = PathBuf::from("/tmp/mmcp-test/missing.toml");
        let source = std::io::Error::new(std::io::ErrorKind::NotFound, "missing.toml");
        let err = StoreError::Io {
            path: path.clone(),
            operation: FileOperation::Read,
            source,
        };

        let chained = err
            .source()
            .and_then(|s| s.downcast_ref::<std::io::Error>())
            .expect("io source must be preserved");
        assert_eq!(chained.kind(), std::io::ErrorKind::NotFound);
        match &err {
            StoreError::Io {
                path: p, operation, ..
            } => {
                assert_eq!(p, &path);
                assert_eq!(*operation, FileOperation::Read);
            }
            other => panic!("expected Io, got {other:?}"),
        }
    }

    #[test]
    fn toml_parse_variant_carries_the_real_source_and_path() {
        let path = PathBuf::from("/tmp/mmcp-test/config.toml");
        let parse_err = toml::from_str::<toml::Value>("not = [valid").unwrap_err();
        let err = StoreError::TomlParse {
            path: path.clone(),
            source: parse_err,
        };

        assert!(
            err.source()
                .and_then(|s| s.downcast_ref::<toml::de::Error>())
                .is_some()
        );
        match &err {
            StoreError::TomlParse { path: p, .. } => assert_eq!(p, &path),
            other => panic!("expected TomlParse, got {other:?}"),
        }
    }

    #[test]
    fn toml_serialize_variant_carries_the_real_source_and_path() {
        // A TOML document's top level must be a table; serializing a
        // bare scalar is the simplest value the `toml` crate's
        // serializer genuinely rejects.
        let path = PathBuf::from("/tmp/mmcp-test/config.toml");
        let ser_err = toml::to_string(&"not a table").unwrap_err();
        let err = StoreError::TomlSerialize {
            path: path.clone(),
            source: ser_err,
        };

        assert!(
            err.source()
                .and_then(|s| s.downcast_ref::<toml::ser::Error>())
                .is_some()
        );
        match &err {
            StoreError::TomlSerialize { path: p, .. } => assert_eq!(p, &path),
            other => panic!("expected TomlSerialize, got {other:?}"),
        }
    }
}
