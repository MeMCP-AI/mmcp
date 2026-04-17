//! `mmcp-store` — programmatic store layer for mmcp consumers.
//!
//! This crate owns the local-first read / write / sync / diagnose
//! logic that every mmcp consumer needs: the `mmcp` CLI binary, the
//! MCP stdio tool surface it serves, the desktop `mmcp-gui`
//! application, and any third-party Rust code driving the store
//! programmatically.
//!
//! The modules are being ported in from `mmcp-client` one at a
//! time. Each module carries an in-file note when it lands here
//! pointing at the git history for the pre-extraction lineage.
//!
//! ## Public surface (WIP)
//!
//! This crate is brand-new — commit 1 of the extraction chain
//! registers it in the workspace with an empty surface. Subsequent
//! commits move the following subsystems in:
//!
//! - `home` — `MmcpHome`, `ResolvedAuthor`, discovery cascade.
//! - `config` — project-config loader (`find_project_root`,
//!   `load`, `save`).
//! - `groups` — `GroupIndex`, `GroupEntry`, refresh loop.
//! - `memory` — typed read / write / edit / delete primitives.
//! - `sync` — thin wrappers around `mmcp-sync` for pull / push.
//! - `diagnostics` — `check_health` / `diagnose` bodies with
//!   typed report structs.
//! - `error` — one `StoreError` enum covering every failure the
//!   store can surface.
//! - `testing` (feature-gated) — tempdir-backed fixtures shared
//!   across consumer crates' integration tests.
//!
//! ## Consumer contract
//!
//! The crate carries zero dependencies on `rmcp`, `clap`,
//! `inquire`, or `egui`. The public surface is typed structs +
//! `thiserror` errors + `tokio` async methods. Each consumer owns
//! its own argument parsing, user prompting, and response
//! serialization; the store owns correctness of the underlying
//! git writes, index coherence, and error shapes.

#![forbid(unsafe_code)]

pub mod config;
pub mod diagnostics;
pub mod error;
pub mod groups;
pub mod home;
pub mod memory;
pub mod sessions;
pub mod sync;

#[cfg(any(test, feature = "testing"))]
pub mod testing;

pub use diagnostics::{
    DiagReport, GroupReport, Issue, diagnose_all, diagnose_group, health_check_all,
    health_check_group,
};
pub use error::StoreError;
pub use groups::{GroupEntry, GroupIndex};
pub use home::{MmcpHome, ResolvedAuthor, read_git_global};
pub use memory::{
    ImportError, ImportResult, SynthFrontmatter, create_memory_file, delete_memory_file,
    import_memory, memory_exists, parse_kind, resolve_group, slugify_filename, update_memory_file,
    validate_slug,
};
pub use sessions::{MemoryRead, SessionState, SessionStore};
pub use sync::{IndexResolver, build_engine};
