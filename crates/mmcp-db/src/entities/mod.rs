//! SeaORM entity definitions for mmcp.
//!
//! Each module holds exactly one table with its `Model`, `Entity`,
//! `Column`, `ActiveModel`, and `Relation` types. Timestamps are
//! stored as `i64` milliseconds since the Unix epoch so both SQLite
//! and Postgres are first-class backends without a chrono dependency.

pub mod group;
pub mod membership;
pub mod memory;
pub mod memory_read;
pub mod memory_version;
pub mod org;
pub mod org_member;
pub mod session;
pub mod user;
