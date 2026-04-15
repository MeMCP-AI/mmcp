//! Sync error type.

use thiserror::Error;
use uuid::Uuid;

/// Failures returned by the sync engine.
#[derive(Debug, Error)]
pub enum SyncError {
    /// The supplied semver string is not parseable.
    #[error("invalid semver: {0}")]
    InvalidVersion(#[from] semver::Error),

    /// The pending queue entry does not exist.
    #[error("pending edit not found: {0}")]
    NotFound(String),

    /// Git backend failure.
    #[error("git error: {0}")]
    Git(#[from] mmcp_git::GitError),

    /// HTTP transport failure while talking to the remote mmcp
    /// server (connection refused, TLS error, bad body, and so on).
    #[error("transport error: {0}")]
    Transport(String),

    /// The remote server answered with a non-success status and a
    /// human-readable reason, but the status was not a conflict.
    #[error("remote error ({status}): {message}")]
    Remote { status: u16, message: String },

    /// Push was rejected because the remote advanced under our
    /// feet. Callers should pull, re-apply the pending edit, and
    /// try again.
    ///
    /// The payload points at the memory that needs resolving plus
    /// the two commit ids so the CLI can print a minimal diff hint.
    #[error("push rejected: remote moved for memory {memory}")]
    Conflict {
        memory: Uuid,
        local_commit: String,
        remote_commit: String,
    },
}

impl SyncError {
    /// Construct a transport error with a formatted message.
    pub(crate) fn transport(e: impl std::fmt::Display) -> Self {
        Self::Transport(e.to_string())
    }
}
