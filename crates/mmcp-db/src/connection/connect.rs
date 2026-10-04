//! [`connect`], which opens the connection from a typed target.

use sea_orm::sqlx::postgres::PgPoolOptions;
use sea_orm::sqlx::sqlite::{SqliteJournalMode, SqlitePoolOptions};
use sea_orm::sqlx::{self, ConnectOptions};
use sea_orm::{SqlxPostgresConnector, SqlxSqliteConnector};

use super::database::Database;
use super::database_target::DatabaseTarget;
use super::pool_policy::{SQLITE_POOL_SIZE, STATEMENT_LOG_LEVEL, TEST_BEFORE_ACQUIRE};
use super::postgres_target::PostgresTarget;
use super::sqlite_target::SqliteTarget;
use crate::defaults::SQLITE_BUSY_TIMEOUT;
use crate::error::DbError;

/// Open a database connection to `target`.
///
/// The pool is built here from the target's own parse, so the URL never reaches SeaORM.
/// SeaORM quotes a URL in its errors and in a trace span of its connect function, which this function never calls.
/// The pool follows the policy of [`super::pool_policy`], which equals what SeaORM's connect applies by default.
///
/// A SQLite connection runs in write-ahead-log journal mode, so a commit does not block readers file-wide.
/// It waits at most [`SQLITE_BUSY_TIMEOUT`] on a locked file.
pub async fn connect(target: &DatabaseTarget) -> Result<Database, DbError> {
    let connection = match target {
        DatabaseTarget::Postgres(postgres) => connect_postgres(postgres).await?,
        DatabaseTarget::Sqlite(sqlite) => connect_sqlite(sqlite).await?,
    };
    Ok(Database::new(connection))
}

async fn connect_postgres(target: &PostgresTarget) -> Result<sea_orm::DatabaseConnection, DbError> {
    let options = target
        .connect_options()
        .map_err(DbError::ConnectionOptionsRejected)?
        .log_statements(STATEMENT_LOG_LEVEL);
    let pool = PgPoolOptions::new()
        .test_before_acquire(TEST_BEFORE_ACQUIRE)
        .connect_with(options)
        .await
        .map_err(|error| match error {
            sqlx::Error::Configuration(_) => target.explain_configuration_error(),
            other => DbError::ConnectionFailed(other),
        })?;
    Ok(SqlxPostgresConnector::from_sqlx_postgres_pool(pool))
}

async fn connect_sqlite(target: &SqliteTarget) -> Result<sea_orm::DatabaseConnection, DbError> {
    let options = target
        .connect_options()
        .map_err(DbError::ConnectionOptionsRejected)?
        .log_statements(STATEMENT_LOG_LEVEL)
        .journal_mode(SqliteJournalMode::Wal)
        .busy_timeout(SQLITE_BUSY_TIMEOUT);
    let pool = SqlitePoolOptions::new()
        .max_connections(SQLITE_POOL_SIZE)
        .test_before_acquire(TEST_BEFORE_ACQUIRE)
        .connect_with(options)
        .await
        .map_err(DbError::ConnectionFailed)?;
    Ok(SqlxSqliteConnector::from_sqlx_sqlite_pool(pool))
}
