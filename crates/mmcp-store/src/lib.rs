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

pub mod archive;
pub mod config;
pub mod diagnostics;
pub mod error;
pub mod features;
pub mod groups;
pub mod home;
pub mod import_adoc;
pub mod lock;
pub mod memory;
pub mod memory_ops;
pub mod issues;
pub mod sessions;
pub mod sync;
pub mod tracker;

#[cfg(any(test, feature = "testing"))]
pub mod testing;

pub use archive::{
    ARCHIVE_FORMAT_VERSION, ArchiveError, ArchiveManifest, ArchiveMode, ArchivedGroupMeta,
    ExportOptions, export_archive,
};
pub use diagnostics::{
    DiagReport, Finding, GroupReport, diagnose_all, diagnose_group, health_check_all,
    health_check_group,
};
pub use error::StoreError;
pub use features::{
    AddSpec as FeatureAddSpec, FeatureError, FeatureRecord, FeatureSummary,
    UpdateSpec as FeatureUpdateSpec, add_feature, delete_feature, list_feature_summaries,
    list_features, read_feature, rename_feature, resolve_project_group, update_feature,
};
// Re-export the shared cross-reference parsers from their owner
// module (`mmcp_core::memory::xrefs`) at the store crate root so
// CLI and MCP consumers do not have to know the upstream path.
// Per global-coding-rules section 13 this re-export lives at the
// crate root (the only place a cross-crate re-export is allowed),
// not on a peer module that would imply ownership.
pub use mmcp_core::memory::{MemoryRefInput, XrefError, parse_cross_refs, parse_memory_refs};
pub use groups::{GroupEntry, GroupIndex};
pub use home::{MmcpHome, ResolvedAuthor, read_git_global};
pub use import_adoc::{ADOC_EXTENSIONS, AdocConvertError, convert_adoc_to_markdown, is_adoc_filename};
pub use memory::{
    AddressingMode, IdValidation, ImportError, ImportResult, MAX_SLUG_LENGTH, MAX_SLUG_SEGMENTS,
    MemoryFileRef, MemoryFrontmatterEntry, MemorySlugDir, MoveMemoryOutcome, ResolvedMemory,
    SynthFrontmatter, delete_file_at_path, import_memory, list_all_memory_files,
    list_memory_slug_dirs, move_memory_path, parse_kind, read_frontmatter,
    read_frontmatters_in_group, resolve_group, resolve_memory, slugify_filename,
    validate_id_mismatch, validate_memory_slug, validate_slug, validate_slug_segment,
    write_file_at_path, write_memory_by_id,
};
pub use memory_ops::{MemoryEditError, MemoryEditOp, apply_ops};
pub use sessions::{MemoryRead, SessionState, SessionStore};
pub use sync::{IndexResolver, build_engine};
