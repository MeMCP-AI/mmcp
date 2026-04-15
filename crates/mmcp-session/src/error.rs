//! Session tracker error type.

use thiserror::Error;

/// Failures returned by the session tracker.
#[derive(Debug, Error)]
pub enum SessionError {
    /// The session referenced by the caller has no row in the
    /// database. Usually means the tracker was asked to operate on
    /// a session before `start_session` ran for it.
    #[error("session not found: {0}")]
    NotFound(String),

    /// I/O failure while reading the transcript file.
    #[error("transcript I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// Underlying database error.
    #[error("database error: {0}")]
    Db(#[from] mmcp_db::DbError),
}
