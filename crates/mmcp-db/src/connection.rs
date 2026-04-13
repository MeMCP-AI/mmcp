//! Database connection handle.

use sea_orm::{ConnectOptions, DatabaseConnection};
use sea_orm_migration::MigratorTrait;

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

/// Open a database connection from a URL.
///
/// Accepts any URL supported by SeaORM's sqlx backend. Typical values:
/// `postgres://user:pass@host/db`, `sqlite://./local.db`,
/// `sqlite::memory:` for tests.
pub async fn connect(database_url: &str) -> Result<Database, DbError> {
    let opts = ConnectOptions::new(database_url.to_string());
    let conn = sea_orm::Database::connect(opts).await?;
    Ok(Database { conn })
}
