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
}
