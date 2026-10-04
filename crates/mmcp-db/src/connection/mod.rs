//! Database connection handle and the function that opens it.

mod connect;
mod database;

pub use connect::connect;
pub use database::Database;
