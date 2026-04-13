//! Error type for the git storage layer.

use thiserror::Error;

/// Failures returned by a [`GitBackend`](crate::GitBackend).
#[derive(Debug, Error)]
pub enum GitError {
    /// The targeted repository does not exist.
    #[error("repository not found: {0}")]
    RepoNotFound(String),

    /// The requested path or file inside the repository was not found.
    #[error("path not found in repository: {0}")]
    PathNotFound(String),

    /// The requested revision could not be resolved.
    #[error("revision not found: {0}")]
    RevNotFound(String),

    /// I/O failure while accessing the repository on disk.
    #[error("git I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// Underlying `gix` error wrapped into a string so our public API
    /// does not depend on `gix` types.
    #[error("gix error: {0}")]
    Gix(String),

    /// The backend is configured but the operation is not supported.
    /// Used by the native backend for network operations that require
    /// the smart HTTP server to be wired up.
    #[error("operation not supported by this backend: {0}")]
    Unsupported(&'static str),

    /// UTF-8 decoding error when reading a text file.
    #[error("invalid UTF-8 in file: {0}")]
    Utf8(#[from] std::string::FromUtf8Error),
}
