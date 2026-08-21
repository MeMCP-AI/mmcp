//! Sync engine orchestrating the git control and content planes.
//!
//! The engine draws a clean line between two concerns:
//!
//! - The **control plane**, which lives in `SyncClient` and talks
//!   JSON over HTTPS to `mmcp-server`. Advertises which
//!   groups exist and at what head commit (`/sync/manifest`,
//!   `/sync/refs/<uuid>`); the bump intent travels on the commit
//!   stream itself.
//! - The **content plane**, which lives in `GitBackend` and moves
//!   actual blobs. `fetch` / `pull` / `push` all delegate their
//!   on-wire work to `backend.fetch` and `backend.push`.
//!
//! The three verbs are intentionally git-symmetric:
//!
//! - `fetch` writes each in-scope group's remote head into
//!   `refs/remotes/origin/main` without advancing local `main`.
//! - `pull` fast-forwards local `main` to the remote head.
//! - `push` ships local `main` to the remote. No per-edit queue;
//!   each memory mutation already commits to the local repo, and
//!   push is just `git push origin main` per group.
//!
//! Tests under `tests/engine_smoke.rs` exercise the engine against
//! a `wiremock` HTTP server plus an in-process native git backend
//! so every path except real network transport is covered without
//! a running `mmcp-server`.
//!
//! Module layout: the `defaults` module holds the shared concurrency
//! cap, the `concurrency` module holds the [`run_bounded`] helper
//! (exported at the crate root so `mmcp-server` can reuse it instead
//! of hand-rolling its own bounded fan-out), the `scope` module holds
//! [`NoScopeIndex`] and the rest of scope-filter matching, the
//! `resolver` module holds [`GroupHandleResolver`], the
//! local-handle-lookup trait, the `remote` module holds
//! [`BoundRemote`] and [`RemoteTransport`], the types describing one
//! engine-bound remote, the `push_scope` module holds [`PushScope`],
//! the selector for how `push` chooses among several bound remotes
//! (its own file: a selector is a distinct concern from a bound
//! remote's own description), the `sync_engine` module holds
//! [`SyncEngine`] itself, its push/pull/fetch orchestration, and the
//! `partition_sync_outcomes` helper the three verbs share, and the
//! `reports` module holds [`SyncReport`] and the rest of the outcome
//! types every verb returns.

mod concurrency;
mod defaults;
mod push_scope;
mod remote;
mod reports;
mod resolver;
mod scope;
mod sync_engine;

pub use concurrency::run_bounded;
pub use push_scope::PushScope;
pub use remote::{BoundRemote, RemoteTransport};
pub use reports::{
    FetchReport, FetchedGroup, GroupSyncFailure, PullReport, PushReport, PushedGroup,
    RemoteManifestFailure, RemotePushOutcome, SyncReport,
};
pub use resolver::GroupHandleResolver;
pub use scope::NoScopeIndex;
pub use sync_engine::SyncEngine;
