//! Database error type.

use thiserror::Error;

/// Failures returned by the database layer.
#[derive(Debug, Error)]
pub enum DbError {
    /// Underlying SeaORM / sqlx error.
    #[error("database error: {0}")]
    Orm(#[from] sea_orm::DbErr),

    /// A row was expected but none was found.
    #[error("record not found")]
    NotFound,

    /// Migration failure.
    #[error("migration error: {0}")]
    Migration(String),

    /// A session's turn counter is already at `i32::MAX` and cannot be
    /// incremented without wrapping. Surfaced as data rather than
    /// silently saturating or wrapping, so a caller sees the counter
    /// stopped advancing instead of silently losing turn-order fidelity.
    #[error("session {session_id} turn counter overflowed at {current}")]
    TurnCounterOverflow { session_id: String, current: i32 },
}
