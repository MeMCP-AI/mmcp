//! Database connection handle.

use sea_orm::sqlx::sqlite::SqliteJournalMode;
use sea_orm::{ConnectOptions, DatabaseConnection};
use sea_orm_migration::MigratorTrait;

use crate::database_url_redaction::redact_database_url;
use crate::defaults::SQLITE_BUSY_TIMEOUT;
use crate::error::DbError;
use crate::migration::Migrator;

/// Wrapper around a SeaORM [`DatabaseConnection`] exposing the
/// migration and teardown helpers mmcp needs.
#[derive(Debug, Clone)]
pub struct Database {
    conn: DatabaseConnection,
}

impl Database {
    /// Borrow the inner SeaORM connection handle.
    #[must_use]
    pub fn connection(&self) -> &DatabaseConnection {
        &self.conn
    }

    /// Take ownership of the inner connection.
    #[must_use]
    pub fn into_connection(self) -> DatabaseConnection {
        self.conn
    }

    /// Run every pending migration against the connected database.
    pub async fn migrate(&self) -> Result<(), DbError> {
        Migrator::up(&self.conn, None)
            .await
            .map_err(|e: sea_orm::DbErr| DbError::Migration(e.to_string()))
    }

    /// Drop every mmcp-managed table. Used by tests and by the
    /// `mmcp server reset` admin command (not yet wired).
    pub async fn reset(&self) -> Result<(), DbError> {
        Migrator::down(&self.conn, None)
            .await
            .map_err(|e: sea_orm::DbErr| DbError::Migration(e.to_string()))
    }
}

/// Schemes the compiled-in SeaORM drivers serve, as SeaORM matches them.
const SUPPORTED_URL_SCHEMES: [&str; 3] = ["postgres", "postgresql", "sqlite"];

/// The two checks SeaORM makes on a URL before it connects, with errors that never quote the URL.
fn check_database_url(database_url: &str) -> Result<(), DbError> {
    let parsed =
        url::Url::parse(database_url).map_err(|source| DbError::DatabaseUrlUnparsable {
            redacted_url: redact_database_url(database_url),
            source,
        })?;
    if SUPPORTED_URL_SCHEMES.contains(&parsed.scheme()) {
        return Ok(());
    }
    Err(DbError::DatabaseUrlSchemeUnsupported {
        redacted_url: redact_database_url(database_url),
        scheme: parsed.scheme().to_owned(),
    })
}

/// Open a database connection from a URL.
///
/// Accepts any URL supported by SeaORM's sqlx backend. Typical values:
/// `postgres://user:pass@host/db`, `sqlite://./local.db`,
/// `sqlite::memory:` for tests.
///
/// A SQLite connection runs in write-ahead-log journal mode, so a commit does not block readers file-wide.
/// It waits at most [`SQLITE_BUSY_TIMEOUT`] on a locked file.
///
/// The URL is checked before SeaORM sees it, because SeaORM quotes the whole URL, password included, in the errors of the two checks it makes itself.
/// A URL that fails a check yields an error holding only [`crate::redact_database_url`] of it.
pub async fn connect(database_url: &str) -> Result<Database, DbError> {
    check_database_url(database_url)?;
    let mut opts = ConnectOptions::new(database_url.to_string());
    opts.map_sqlx_sqlite_opts(|sqlite_opts| {
        sqlite_opts
            .journal_mode(SqliteJournalMode::Wal)
            .busy_timeout(SQLITE_BUSY_TIMEOUT)
    });
    let conn = sea_orm::Database::connect(opts).await?;
    Ok(Database { conn })
}
