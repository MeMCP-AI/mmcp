//! Database error type.

use thiserror::Error;
use uuid::Uuid;

/// Failures returned by the database layer.
#[derive(Debug, Error)]
pub enum DbError {
    /// Underlying SeaORM / sqlx error.
    #[error("database error: {0}")]
    Orm(#[from] sea_orm::DbErr),

    /// The driver rejected the connection options that a database target spells.
    /// The parse errors left after the target's own checks carry no value of the URL.
    #[error("the database connection options were rejected")]
    ConnectionOptionsRejected(#[source] sea_orm::sqlx::Error),

    /// The connection to the database could not be established.
    #[error("the database connection could not be established")]
    ConnectionFailed(#[source] sea_orm::sqlx::Error),

    /// The password is not valid under SASLprep, which SCRAM authentication needs.
    /// The driver error that names the character is dropped.
    #[error("the database password holds a character that SASLprep does not allow")]
    PasswordRejectedBySaslprep,

    /// The user name is not valid under SASLprep, which SCRAM authentication needs.
    /// The driver error that names the character is dropped.
    #[error("the database user name holds a character that SASLprep does not allow")]
    UsernameRejectedBySaslprep,

    /// The driver rejected the connection configuration for a reason this crate does not know.
    /// The driver error is dropped, since it may quote a value.
    #[error("the database connection configuration was rejected")]
    ConnectionConfigurationRejected,

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
