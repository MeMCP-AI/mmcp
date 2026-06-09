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
//! Memory bytes are copied verbatim from HEAD, so UUIDs, slugs,
//! kinds, tags, feature/issue numbers, refs, and source survive a
//! round trip. Feature and issue memories ride along as ordinary
//! memory files. The export path writes the stream; the import path
//! reads it and replays each memory through the same `import_memory`
//! primitive the loose-file import path uses.

pub mod error;
pub mod manifest;

pub use error::ArchiveError;
pub use manifest::{
    ARCHIVE_FORMAT_VERSION, ARCHIVE_GROUPS_DIR, ARCHIVE_MANIFEST_FILENAME, ArchiveManifest,
    ArchiveMode, ArchivedGroupMeta,
};
