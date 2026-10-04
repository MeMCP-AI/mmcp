//! [`Database`], the connected handle with its migration helpers.

use sea_orm::DatabaseConnection;
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
    pub(super) fn new(conn: DatabaseConnection) -> Self {
        Self { conn }
    }

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
