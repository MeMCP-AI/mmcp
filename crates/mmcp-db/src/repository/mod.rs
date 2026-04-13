//! Repository helpers layered over the raw SeaORM entities.
//!
//! Each repository targets one table and returns [`DbError`](crate::DbError)
//! results. Keeps the call sites in `mmcp-server` and `mmcp-client`
//! free of SeaORM boilerplate.

pub mod group_repo;
pub mod memory_repo;
pub mod org_repo;
pub mod session_repo;
pub mod user_repo;
