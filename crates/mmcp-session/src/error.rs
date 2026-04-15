//! Session error type.
//!
//! This crate keeps only the pure compaction detection primitives
//! used by `mmcp-client`'s on-disk `SessionStore`. Persistence
//! concerns (file I/O, serialization, database access) live in
//! the consuming crate, so the error surface here is intentionally
//! minimal.

use thiserror::Error;

/// Failures returned by [`compute_signature`](crate::compute_signature).
#[derive(Debug, Error)]
pub enum SessionError {
    /// I/O failure while reading the transcript file on disk.
    #[error("transcript I/O error: {0}")]
    Io(#[from] std::io::Error),
}
