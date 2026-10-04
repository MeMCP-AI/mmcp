//! Default values owned by the database layer.

use std::time::Duration;

/// How long a SQLite connection waits on a locked database before the
/// statement fails with a busy error.
/// A write contends with other in-process readers and writers on the same file; this bounds the wait.
pub const SQLITE_BUSY_TIMEOUT: Duration = Duration::from_secs(5);
