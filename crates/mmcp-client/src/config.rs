//! Project-config shim — moved to `mmcp-store` as part of FR-020.
//!
//! This module re-exports the loader surface from
//! `mmcp_store::config` so existing `crate::config::…` call sites
//! keep compiling during the extraction chain. The shim disappears
//! in commit 8 when `mmcp-client/src/lib.rs` is deleted and every
//! consumer updates its imports to `mmcp_store::config`.

pub use mmcp_store::config::{PROJECT_MANIFEST, config_path_for, find_project_root, load, save};
