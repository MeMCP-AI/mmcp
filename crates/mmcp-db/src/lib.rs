//! Database layer for mmcp.
//!
//! Holds SeaORM entity definitions, migrations, and shared query helpers.
//! The same entities target Postgres in `mmcp-server` and SQLite in the
//! local mirror kept by `mmcp-client`, with the backend selected at runtime
//! by the scheme of a database URL, parsed once into a [`DatabaseTarget`].

pub mod connection;
mod database_url_redaction;
mod defaults;
pub mod entities;
pub mod error;
pub mod migration;
pub mod repository;

pub use connection::{
    Database, DatabaseTarget, DatabaseUrlError, PostgresTarget, SqliteTarget, connect,
};
pub use database_url_redaction::redact_database_url;
pub use defaults::SQLITE_BUSY_TIMEOUT;
pub use error::DbError;

/// Re-export of the SeaORM connection type so downstream crates do
/// not need a direct dependency on `sea-orm`.
pub type Connection = sea_orm::DatabaseConnection;
