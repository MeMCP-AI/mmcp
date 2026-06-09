//! Portable group archives.
//!
//! An mmcp archive is a single `tar` stream (optionally gzip-
//! compressed) whose internal layout mirrors the on-disk repo layout
//! minus git internals, plus a root table of contents:
//!
//! ```text
//! archive.toml                                   table of contents
//! groups/<group-uuid>/.mmcp.toml                 verbatim group manifest
//! groups/<group-uuid>/memories/<slug>/<id>.md    verbatim memory file
//! ```
//!
//! Export copies HEAD bytes verbatim. Import replays each memory
//! through the same `import_memory` primitive the loose-file import
//! path uses, which preserves the modeled frontmatter (id, slug,
//! kind, tags, feature/issue numbers, refs, source) and the body but
//! re-renders in canonical form — so identities round trip, while
//! exact byte formatting and any unmodeled frontmatter keys do not.
//! Feature and issue memories ride along as ordinary memory files.

pub mod error;
pub mod export;
pub mod filter;
pub mod import;
pub mod manifest;

pub use error::ArchiveError;
pub use filter::{MemoryFilter, parse_memory_kind};
pub use export::{ExportOptions, collect_group_tags, export_archive, export_archive_to_path};
pub use import::{
    ArchiveGroupListing, GroupImportOutcome, ImportArchiveOptions, ImportArchiveReport,
    MemoryConflict, import_archive, inspect_archive, list_archive,
};
pub use manifest::{
    ARCHIVE_FORMAT_VERSION, ARCHIVE_GROUPS_DIR, ARCHIVE_MANIFEST_FILENAME, ArchiveManifest,
    ArchiveMode, ArchivedGroupMeta,
};
