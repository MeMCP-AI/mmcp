//! [`StoreError`], the unified error type for the `mmcp-store` public surface.
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

use super::FileOperation;

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

    /// The project- or user-level config at `path` declared two
    /// `sync.remotes` entries sharing the same `name`. Single-file
    /// semantic-validation failure, distinct from a TOML syntax
    /// error: see `mmcp_core::config::ConfigError::DuplicateRemoteName`,
    /// which this variant wraps with the failing file's path attached.
    #[error("duplicate sync remote name in {path}: {name}", path = path.display())]
    ConfigDuplicateRemoteName {
        /// Path of the config file that declared the collision.
        path: PathBuf,
        /// The name shared by two or more `remotes` entries.
        name: String,
    },

    /// The project- or user-level config at `path` marked more than
    /// one `sync.remotes` entry `default = true`. Single-file
    /// semantic-validation failure; see
    /// `mmcp_core::config::ConfigError::MultipleDefaultRemotes`.
    #[error(
        "more than one default sync remote in {path}: {joined}",
        path = path.display(),
        joined = names.join(", ")
    )]
    ConfigMultipleDefaultRemotes {
        /// Path of the config file that declared the collision.
        path: PathBuf,
        /// Names of every remote in this file marked `default = true`.
        names: Vec<String>,
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

    /// The effective remote set (user + project `[sync]` merged)
    /// declares the same remote `name` more than once. Cross-level
    /// duplicate, or a declared remote colliding with the synthetic
    /// name a `server_url` legacy shorthand entry resolves to; the
    /// single-file, same-level case is already rejected earlier by
    /// `mmcp_core::config::ConfigError::DuplicateRemoteName`.
    #[error("sync remote name `{name}` is declared more than once across user and project config")]
    RemoteNameCollision {
        /// The name shared by two or more resolved remotes.
        name: String,
    },

    /// A user-level `direct-git` remote declared no `group`. Unlike
    /// a project-level entry, there is no implicit default: a
    /// group-less user-level `direct-git` remote would push whichever
    /// project happens to be active into the same shared repo.
    #[error(
        "direct-git remote `{name}` at user level has no group; group is required at user level"
    )]
    DirectGitMissingGroup {
        /// Name of the offending remote.
        name: String,
    },

    /// The effective remote set has two or more remotes and none is
    /// marked `default = true` at any level.
    #[error(
        "ambiguous default sync remote: mark exactly one of [{}] as default = true",
        candidates.join(", ")
    )]
    AmbiguousDefaultRemote {
        /// Every remote name in the effective set, in resolution order.
        candidates: Vec<String>,
    },

    /// A remote's declared `name` is empty or contains a character
    /// outside ASCII alphanumerics, `-`, and `_`. Remote names flow
    /// verbatim into a git ref path (`refs/remotes/<name>/main`), so
    /// an unconstrained name could produce a malformed or
    /// path-traversing refspec.
    #[error("sync remote name `{name}` must be non-empty ASCII alphanumeric, `-`, or `_`")]
    InvalidRemoteName {
        /// The offending name.
        name: String,
    },

    /// A `direct-git` remote's resolved group reference (explicit
    /// `group` field, or the project's own UUID when defaulted) does
    /// not resolve to any group the local mirror knows about.
    #[error(
        "direct-git remote `{remote_name}` group `{group_ref}` does not resolve to a mirrored group: {source}"
    )]
    DirectGitGroupNotFound {
        /// Name of the offending remote.
        remote_name: String,
        /// The unresolved group reference (UUID or slug) as declared
        /// or defaulted.
        group_ref: String,
        /// The underlying group-lookup failure.
        #[source]
        source: crate::memory::ImportError,
    },

    /// A first pull could not safely adopt its configured repository.
    #[error("cannot bootstrap direct-git remote {remote_name}: {reason}")]
    DirectGitBootstrap {
        /// Configured remote name.
        remote_name: String,
        /// Identity, manifest, or destination validation failure.
        reason: String,
    },

    /// A pull selector has no matching local or configured group.
    #[error("group not found: {query}")]
    PullGroupNotFound {
        /// User-supplied UUID or slug.
        query: String,
    },

    /// A test fixture could not create its scratch directory under
    /// the OS temp root.
    ///
    /// `tempfile::TempDir::new` tries several randomly named
    /// candidates internally and does not expose which specific one
    /// failed, so `root` names the OS temp root the attempt happened
    /// under, not the candidate path itself; do not read `root` as
    /// the exact path that failed.
    #[error("failed to create a temporary directory under {root}: {source}", root = root.display())]
    TempDirUnavailable {
        /// The OS temp root (`std::env::temp_dir()`) the failed attempt happened under.
        root: PathBuf,
        /// The underlying OS error from the last failed attempt `tempfile` made.
        #[source]
        source: std::io::Error,
    },
}

impl StoreError {
    /// Build the [`StoreError::Io`] variant.
    ///
    /// Every filesystem-failure call site across this crate
    /// constructs the same three fields (`path`, `operation`,
    /// `source`); this constructor is their single owning
    /// definition, so a shape change to the variant only has one
    /// call site to update instead of the bare struct literal
    /// repeated at every `map_err`.
    pub fn io(path: impl Into<PathBuf>, operation: FileOperation, source: std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            operation,
            source,
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
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
    fn io_constructor_builds_the_same_shape_as_the_bare_literal() {
        let path = PathBuf::from("/tmp/mmcp-test/missing.toml");
        let source = std::io::Error::new(std::io::ErrorKind::NotFound, "missing.toml");
        let err = StoreError::io(path.clone(), FileOperation::Read, source);

        match &err {
            StoreError::Io {
                path: p, operation, ..
            } => {
                assert_eq!(p, &path);
                assert_eq!(*operation, FileOperation::Read);
            }
            other => panic!("expected Io, got {other:?}"),
        }
        assert!(
            err.source()
                .and_then(|s| s.downcast_ref::<std::io::Error>())
                .is_some(),
            "io() must preserve the real std::io::Error as the source"
        );
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

    #[test]
    fn temp_dir_unavailable_names_the_root_not_a_specific_candidate_path() {
        let root = PathBuf::from("/tmp");
        let source = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "denied");
        let err = StoreError::TempDirUnavailable {
            root: root.clone(),
            source,
        };

        assert!(
            err.source()
                .and_then(|s| s.downcast_ref::<std::io::Error>())
                .is_some(),
            "source must be the real std::io::Error, not a stringified copy"
        );
        // The message must talk about the root as "under", never as
        // "the" failed path, so it does not claim knowledge tempfile
        // never gave us.
        assert!(err.to_string().contains("under /tmp"));
        match &err {
            StoreError::TempDirUnavailable { root: r, .. } => assert_eq!(r, &root),
            other => panic!("expected TempDirUnavailable, got {other:?}"),
        }
    }
}
