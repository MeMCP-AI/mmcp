//! Error type for the git storage layer.

use thiserror::Error;

/// Strip basic-auth credentials (`https://user:pass@host/...`) from a
/// remote URL so stderr and error messages never leak them into logs.
///
/// Keeps the path portion intact so the forge + repo are still visible.
/// Falls back to the original string if parsing fails.
fn redact_url(url: &str) -> String {
    if let Some((scheme, rest)) = url.split_once("://")
        && let Some(at_idx) = rest.find('@')
    {
        return format!("{scheme}://<redacted>@{}", &rest[at_idx + 1..]);
    }
    url.to_string()
}

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

    /// I/O failure while accessing the repository on disk, or while
    /// spawning the `git` subprocess for `clone`/`fetch`/`push`.
    #[error("git I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// Failed to open or initialize the bare repository at `path`.
    /// Wraps the underlying `gix` error so the source chain survives
    /// without exposing `gix` types in this crate's public API.
    #[error("failed to open repository at {path}: {source}")]
    OpenRepo {
        path: String,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    /// Failed to resolve a revision to a commit, or to walk the commit
    /// graph (ancestry checks, history walks).
    #[error("failed to resolve revision: {source}")]
    ResolveRev {
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    /// Failed to read a blob, or to descend a tree while locating one.
    #[error("failed to read blob: {source}")]
    ReadBlob {
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    /// Failed to build or write a new commit object.
    #[error("failed to write commit: {source}")]
    Commit {
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    /// Failed to create or update a ref: a branch, a tag, or the
    /// target of a fast-forward.
    #[error("failed to update ref {name}: {source}")]
    RefUpdate {
        name: String,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    /// Failed to decode or encode the group `.mmcp.toml` manifest.
    /// `operation` names the direction (`"decode"` or `"encode"`).
    #[error("failed to {operation} manifest: {source}")]
    Manifest {
        operation: &'static str,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    /// The blocking task driving a `gix` call panicked or was cancelled.
    #[error("git task join error: {0}")]
    TaskJoin(#[from] tokio::task::JoinError),

    /// The backend is configured but the operation is not supported.
    /// Used by the native backend for network operations that require
    /// the smart HTTP server to be wired up.
    #[error("operation not supported by this backend: {0}")]
    Unsupported(&'static str),

    /// Network / remote transport failure while talking to a git
    /// remote. The native backend returns this when the `git`
    /// subprocess driving `fetch`, `push`, or `clone` exits
    /// non-zero: typically because the remote does not exist,
    /// refused the connection, or rejected the ref update. The sync
    /// engine treats this as a "content plane deferred" signal so a
    /// successful control-plane push is still reported even when
    /// bytes cannot reach the remote yet.
    ///
    /// Carries the operation name (`clone`/`fetch`/`push`/…), the
    /// redacted remote URL, and the subprocess's stderr so callers
    /// can render targeted diagnostics against any forge.
    #[error("git {op} against {url} failed: {stderr}")]
    Transport {
        op: &'static str,
        url: String,
        stderr: String,
    },

    /// UTF-8 decoding error when reading a text file. Every
    /// `GitBackend::read_file` implementation returns a borrowed
    /// `Bytes` buffer, not an owned `Vec<u8>`, so callers validate
    /// via `std::str::from_utf8` (borrowing) rather than
    /// `String::from_utf8` (consuming); this variant wraps that
    /// error type to match.
    #[error("invalid UTF-8 in file: {0}")]
    Utf8(#[from] std::str::Utf8Error),
}

impl GitError {
    /// Build a `Transport` variant, redacting any basic-auth credentials
    /// from the URL. Callers pass the subprocess stderr verbatim.
    #[must_use]
    pub fn transport(op: &'static str, url: &str, stderr: impl Into<String>) -> Self {
        GitError::Transport {
            op,
            url: redact_url(url),
            stderr: stderr.into(),
        }
    }
}
