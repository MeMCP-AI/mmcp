//! Sync engine for mmcp.
//!
//! Two layers, clearly separated:
//!
//! - The **control plane**: a typed `SyncClient` wrapping the
//!   HTTP endpoints that `mmcp-server` serves under `/sync/*`.
//!   Advertises which groups exist and at what head commit; the
//!   bump intent travels on the commit itself (see
//!   [`bump::parse_bump_intent`]).
//! - The **content plane**: a `GitBackend` handle that moves the
//!   actual git objects. The engine calls `backend.push`,
//!   `backend.fetch`, and `backend.fast_forward`: fetch populates
//!   the local tracking ref, pull fast-forwards from it, push
//!   ships local `main` to the remote.
//!
//! `SyncEngine` orchestrates the four verbs: fetch, pull, push,
//! sync. Tests drive the engine through `wiremock` + an in-process
//! native backend so every path except real network transport is
//! covered without a running `mmcp-server`.

pub mod bump;
pub mod client;
pub mod engine;
pub mod error;
pub mod filter;
pub mod version;

pub use bump::parse_bump_intent;
pub use client::{
    ConflictBody, ManifestResponse, PushRequest, PushResponse, RefEntry, RefsResponse, RemoteGroup,
    SyncClient,
};
pub use engine::{
    BoundRemote, FetchReport, FetchedGroup, GroupHandleResolver, GroupSyncFailure, PullReport,
    PushReport, PushScope, PushedGroup, RemoteManifestFailure, RemotePushOutcome, RemoteTransport,
    SyncEngine, SyncReport, run_bounded,
};
pub use error::SyncError;
pub use filter::{ScopeIndex, SyncFilter};
pub use version::negotiate_next_version;
