//! Database layer for mmcp.
//!
//! Holds SeaORM entity definitions, migrations, and shared query helpers.
//! The same entities target Postgres in `mmcp-server` and SQLite in the
//! local mirror kept by `mmcp-client`, with the backend selected at runtime
//! from a `database_url`.

pub mod connection;
pub mod entities;
pub mod error;
pub mod migration;
pub mod repository;

pub use connection::{Database, connect};
pub use error::DbError;
