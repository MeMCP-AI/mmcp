//! Error type for the git storage layer.

use thiserror::Error;

/// Strip basic-auth credentials (`https://user:pass@host/...`) from a
/// remote URL so stderr and error messages never leak them into logs.
///
/// Keeps the path portion intact so the forge + repo are still visible.
/// Falls back to the original string if parsing fails.
fn redact_url(url: &str) -> String {
    if let Some(rest) = url.split_once("://").map(|(_, r)| r)
        && let Some(at_idx) = rest.find('@')
    {
        let (scheme, _) = url.split_once("://").expect("checked above");
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

    /// UTF-8 decoding error when reading a text file.
    #[error("invalid UTF-8 in file: {0}")]
    Utf8(#[from] std::string::FromUtf8Error),
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
