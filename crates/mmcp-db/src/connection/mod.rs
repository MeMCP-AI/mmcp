//! Database connection handle, the typed database target and the function that opens it.

mod at_sign_check;
mod connect;
mod database;
mod database_target;
mod database_url_error;
mod pool_policy;
mod postgres_parameters;
mod postgres_target;
mod sqlite_target;

pub use connect::connect;
pub use database::Database;
pub use database_target::DatabaseTarget;
pub use database_url_error::DatabaseUrlError;
pub use postgres_target::PostgresTarget;
pub use sqlite_target::SqliteTarget;
