//! Client-side runtime state glue.
//!
//! Everything that used to live here — `SessionStore`, `GroupIndex`,
//! `StateError` — moved into `mmcp-store` as part of FR-020 so the
//! desktop GUI and any third-party consumer can drive the stores
//! without depending on the CLI binary.
//!
//! What remains is strictly client-specific plumbing that `mmcp-store`
//! should not own:
//!
//! - [`spawn_watcher`] / [`WatcherHandle`]: a `notify`-backed
//!   filesystem watcher the serve process uses to keep its in-memory
//!   [`GroupIndex`](mmcp_store::groups::GroupIndex) coherent with
//!   bare-repo churn on disk. The watcher is a thin wrapper around
//!   the store's public refresh API and is meaningful only in a
//!   long-running CLI/server context, so it stays here rather than
//!   forcing `notify` into every `mmcp-store` consumer.

mod watcher;

pub use watcher::{WatcherHandle, spawn_watcher};
