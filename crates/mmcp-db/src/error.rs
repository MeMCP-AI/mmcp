//! Database error type.

use thiserror::Error;
use uuid::Uuid;

/// Failures returned by the database layer.
#[derive(Debug, Error)]
pub enum DbError {
    /// Underlying SeaORM / sqlx error.
    /// Built only by the conversion from [`sea_orm::DbErr`], which keeps a driver configuration error out of it.
    #[error("database error: {0}")]
    Orm(#[source] sea_orm::DbErr),

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

    /// Only one of the client certificate and the client key is given, in the URL and in `PGSSLCERT` and `PGSSLKEY` together.
    /// The driver needs both for a TLS handshake with client authentication.
    #[error(
        "the database connection has a client certificate or a client key without the other; give both, or neither, in the URL or in PGSSLCERT and PGSSLKEY"
    )]
    ClientCertificateAndKeyNotPaired,

    /// The client key could not be read as a private key.
    /// The driver error is dropped, because it may quote a line of the key file.
    #[error("the database client key could not be read as a private key in PEM form")]
    ClientKeyRejected,

    /// The driver rejected the connection configuration, at the first connection or at a later one, for a reason this crate does not know.
    /// The driver error is dropped, since it may quote a value.
    #[error("the database connection configuration was rejected")]
    ConnectionConfigurationRejected,

    /// A row was expected but none was found.
    #[error("record not found")]
    NotFound,

    /// Migration failure, with the SeaORM error as its source.
    /// Built by [`DbError::migration`], which keeps a driver configuration error out of it.
    #[error("migration error: {0}")]
    Migration(#[source] sea_orm::DbErr),

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

impl From<sea_orm::DbErr> for DbError {
    /// The one conversion from a SeaORM error, so every repository crosses it.
    ///
    /// A pool opens a new connection whenever it has none idle, and the driver configuration error of that connection can quote a credential.
    /// SeaORM wraps a driver error as a connection, execution or query error, so each of the three is matched by its variant, never by its text.
    /// Such an error becomes [`DbError::ConnectionConfigurationRejected`] without its source.
    fn from(error: sea_orm::DbErr) -> Self {
        if carries_configuration_error(&error) {
            Self::ConnectionConfigurationRejected
        } else {
            Self::Orm(error)
        }
    }
}

impl DbError {
    /// The error of a migration run, which crosses the same boundary as every repository error.
    pub(crate) fn migration(error: sea_orm::DbErr) -> Self {
        match Self::from(error) {
            Self::Orm(error) => Self::Migration(error),
            other => other,
        }
    }
}

fn carries_configuration_error(error: &sea_orm::DbErr) -> bool {
    use sea_orm::sqlx::Error::Configuration;
    use sea_orm::{DbErr, RuntimeErr};
    matches!(
        error,
        DbErr::Conn(RuntimeErr::SqlxError(source))
            | DbErr::Exec(RuntimeErr::SqlxError(source))
            | DbErr::Query(RuntimeErr::SqlxError(source))
            if matches!(source.as_ref(), Configuration(_))
    )
}

#[cfg(test)]
mod tests {
    use std::error::Error;
    use std::sync::Arc;

    use sea_orm::sqlx::Error::{Configuration, PoolTimedOut};
    use sea_orm::{DbErr, RuntimeErr};

    use super::*;

    const SENTINEL: &str = "SENTINEL_CONFIGURATION_TEXT";

    fn sqlx_error(error: sea_orm::sqlx::Error) -> RuntimeErr {
        RuntimeErr::SqlxError(Arc::new(error))
    }

    #[test]
    fn a_configuration_error_in_any_seaorm_wrapper_loses_its_text_and_its_source() {
        for wrapped in [
            DbErr::Conn(sqlx_error(Configuration(SENTINEL.into()))),
            DbErr::Exec(sqlx_error(Configuration(SENTINEL.into()))),
            DbErr::Query(sqlx_error(Configuration(SENTINEL.into()))),
        ] {
            let error = DbError::from(wrapped);

            assert!(matches!(error, DbError::ConnectionConfigurationRejected));
            assert!(error.source().is_none());
            assert!(!format!("{error} | {error:?}").contains(SENTINEL));
        }
    }

    #[test]
    fn a_migration_error_keeps_its_source_unless_it_is_a_configuration_error() {
        let kept = DbError::migration(DbErr::Custom(SENTINEL.to_owned()));
        let dropped = DbError::migration(DbErr::Conn(sqlx_error(Configuration(SENTINEL.into()))));

        assert!(matches!(kept, DbError::Migration(_)));
        assert!(kept.source().is_some());
        assert!(matches!(dropped, DbError::ConnectionConfigurationRejected));
        assert!(!format!("{dropped} | {dropped:?}").contains(SENTINEL));
    }

    #[test]
    fn every_other_seaorm_error_stays_an_orm_error() {
        for other in [
            DbErr::Conn(sqlx_error(PoolTimedOut)),
            DbErr::Exec(sqlx_error(PoolTimedOut)),
            DbErr::Query(RuntimeErr::Internal(SENTINEL.to_owned())),
            DbErr::Custom(SENTINEL.to_owned()),
        ] {
            assert!(matches!(DbError::from(other), DbError::Orm(_)));
        }
    }
}
