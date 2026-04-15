//! Sync error type.

use thiserror::Error;

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

    /// Database failure.
    #[error("database error: {0}")]
    Db(#[from] mmcp_db::DbError),
}
