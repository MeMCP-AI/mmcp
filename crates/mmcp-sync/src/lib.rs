//! Sync engine for mmcp.
//!
//! Two layers, clearly separated:
//!
//! - The **control plane**: a typed `SyncClient` wrapping the
//!   HTTP endpoints that `mmcp-server` serves under `/sync/*`.
//!   It lists the caller's groups, reads advertised refs, and
//!   registers version bumps when local edits ship.
//! - The **content plane**: a `GitBackend` handle that moves the
//!   actual git objects. The engine calls `backend.push` and
//!   `backend.fetch` once the control-plane decision has been
//!   recorded, so content transfer and metadata registration
//!   stay in lockstep.
//!
//! `SyncEngine` orchestrates both. Tests drive the engine through
//! `wiremock` + an in-process native backend so every path except
//! real network transport is covered without a running
//! `mmcp-server`.
//!
//! The crate also holds the pending push queue (`PendingQueue`)
//! and the pure version bump math (`negotiate_next_version`)
//! used by the server when it assigns a new version to an edit.

pub mod client;
pub mod engine;
pub mod error;
pub mod filter;
pub mod pending;
pub mod version;

pub use client::{
    ConflictBody, ManifestResponse, PushRequest, PushResponse, RefEntry, RefsResponse,
    RemoteGroup, SyncClient,
};
pub use engine::{
    DrainedPush, GroupHandleResolver, PullReport, PushReport, SyncEngine, SyncReport,
};
pub use error::SyncError;
pub use filter::{ScopeIndex, SyncFilter};
pub use pending::{PendingEdit, PendingQueue};
pub use version::negotiate_next_version;
