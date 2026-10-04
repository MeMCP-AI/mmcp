//! Database error type.

use thiserror::Error;
use uuid::Uuid;

/// Failures returned by the database layer.
#[derive(Debug, Error)]
pub enum DbError {
    /// Underlying SeaORM / sqlx error.
    #[error("database error: {0}")]
    Orm(#[from] sea_orm::DbErr),

    /// The database URL cannot be parsed.
    /// `redacted_url` is [`crate::redact_database_url`] of it, never the raw text.
    #[error("the database URL, shown redacted as `{redacted_url}`, cannot be parsed")]
    DatabaseUrlUnparsable {
        redacted_url: String,
        #[source]
        source: url::ParseError,
    },

    /// The database URL names a scheme that no compiled-in driver serves.
    /// `redacted_url` is [`crate::redact_database_url`] of it, never the raw text.
    #[error(
        "the database URL `{redacted_url}` names the scheme `{scheme}`, which no driver serves"
    )]
    DatabaseUrlSchemeUnsupported {
        redacted_url: String,
        scheme: String,
    },

    /// A row was expected but none was found.
    #[error("record not found")]
    NotFound,

    /// Migration failure.
    #[error("migration error: {0}")]
    Migration(String),

    /// A credential was written for a user that does not exist.
    /// The write is rolled back with its epoch increment.
    #[error("credential owner {user_id} does not exist")]
    CredentialOwnerMissing { user_id: Uuid },

    /// A credential write was refused.
    /// The owner's credential epoch moved after the acting session was verified under `verified_epoch`.
    #[error(
        "credential epoch of user {user_id} moved since the session verified epoch {verified_epoch}"
    )]
    CredentialEpochChanged { user_id: Uuid, verified_epoch: i64 },

    /// A session's turn counter is already at `i32::MAX` and cannot be
    /// incremented without wrapping. Surfaced as data rather than
    /// silently saturating or wrapping, so a caller sees the counter
    /// stopped advancing instead of silently losing turn-order fidelity.
    #[error("session {session_id} turn counter overflowed at {current}")]
    TurnCounterOverflow { session_id: String, current: i32 },
}
