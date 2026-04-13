//! Top-level memory record.

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::id::{GroupId, MemoryId};
use crate::memory::{MemoryFrontmatter, MemoryKind};

/// A memory entry inside a group.
///
/// This is the in-memory representation of one Markdown file. The
/// frontmatter block holds user-visible metadata, while the fields on
/// `Memory` itself hold system-managed state that never lives in the
/// file on disk (database identifiers, group ownership, timestamps
/// maintained by the control plane).
///
/// The body is stored separately from the frontmatter so that
/// retrieval code can attach staleness warnings and other wrappers
/// without copying the full Markdown text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Memory {
    /// Stable system-managed identifier.
    pub id: MemoryId,

    /// Group this memory lives in. The group's repo holds the file.
    pub group: GroupId,

    /// File name (without extension) inside the group's repo. Used as
    /// the primary addressable name from MCP tool calls.
    pub slug: String,

    /// User-visible metadata parsed from the frontmatter block.
    pub frontmatter: MemoryFrontmatter,

    /// Markdown body following the closing `+++` fence.
    pub body: String,

    /// When the memory was first created in this group. Preserved
    /// across edits.
    pub created_at: Timestamp,

    /// When the memory was most recently updated on the canonical
    /// branch.
    pub updated_at: Timestamp,
}

impl Memory {
    /// Convenience accessor for the memory's kind.
    #[must_use]
    pub const fn kind(&self) -> MemoryKind {
        self.frontmatter.kind
    }

    /// True if this memory must be read at least once per session.
    #[must_use]
    pub const fn is_mandatory(&self) -> bool {
        self.frontmatter.mandatory
    }
}
