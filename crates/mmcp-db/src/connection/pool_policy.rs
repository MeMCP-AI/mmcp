//! The pool policy of a database connection.
//!
//! Each value equals what SeaORM's own connect applies with its default options.
//! Pools are built here, so that policy is owned here and does not move with a SeaORM release.

use log::LevelFilter;

/// The level at which sqlx logs each statement.
pub(super) const STATEMENT_LOG_LEVEL: LevelFilter = LevelFilter::Info;

/// The size of a SQLite pool, since a SQLite database admits one writer.
pub(super) const SQLITE_POOL_SIZE: u32 = 1;

/// Whether a connection is pinged before the pool hands it out.
pub(super) const TEST_BEFORE_ACQUIRE: bool = true;
