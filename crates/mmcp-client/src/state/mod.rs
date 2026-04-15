//! Client-side runtime state.
//!
//! Holds two independent stores that together replace the SQLite
//! mirror used in earlier builds:
//!
//! - [`SessionStore`] keeps per-session state in flat TOML files
//!   under `~/.mmcp/sessions/`. The hook process and the serve
//!   process share these files through atomic temp-file-rename
//!   writes.
//! - [`GroupIndex`] is an in-memory map from [`GroupId`] to the
//!   bare repository that backs each group. Built on startup by
//!   walking `~/.mmcp/repos/` and kept live by a file-system
//!   watcher (see `watcher` below) so new repositories and
//!   configuration edits are picked up without restarting the
//!   serve process.
//!
//! Neither store opens a database. The client reads memory content
//! directly from git via `mmcp-git`.

mod error;
mod groups;
mod sessions;
mod watcher;

pub use groups::{GroupEntry, GroupIndex};
pub use sessions::SessionStore;
pub use watcher::{WatcherHandle, spawn_watcher};
