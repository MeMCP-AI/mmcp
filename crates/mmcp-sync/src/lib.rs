//! Sync engine for mmcp.
//!
//! Owns two bounded responsibilities today:
//!
//! - The **pending push queue**: an in-process list of local edits
//!   waiting to be pushed to the remote server. Clients enqueue
//!   edits as soon as they are committed locally and the sync task
//!   drains the queue the next time the server is reachable.
//! - The **version bump negotiator**: the pure function that turns
//!   a current canonical version plus an editor-supplied
//!   [`BumpIntent`](mmcp_core::memory::BumpIntent) into the next
//!   version number the server should assign.
//!
//! Actual git push operations are delegated to a
//! [`GitBackend`](mmcp_git::GitBackend) implementation supplied by
//! the caller, so the sync engine stays backend-agnostic.

pub mod error;
pub mod pending;
pub mod version;

pub use error::SyncError;
pub use pending::{PendingEdit, PendingQueue};
pub use version::negotiate_next_version;
