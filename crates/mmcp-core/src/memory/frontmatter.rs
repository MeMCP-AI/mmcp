//! Typed representation of the TOML frontmatter block that heads each
//! memory file.

use serde::{Deserialize, Serialize};

use crate::memory::{BumpIntent, MemoryKind};

/// User-visible metadata written in the `+++`-delimited TOML block at
/// the top of a memory file.
///
/// Every field except `name`, `description`, and `kind` is optional.
/// Unknown fields encountered on disk are preserved by the parser and
/// written back out verbatim, so future mmcp versions can add fields
/// without breaking older clients.
///
/// The `version` field is server-managed. Clients do not hand-edit it;
/// the server assigns it at push time using the
/// [`BumpIntent`](crate::memory::BumpIntent) that accompanied the
/// commit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryFrontmatter {
    /// Human-readable title displayed in listings and the WebUI.
    pub name: String,

    /// One-line summary used by the AI for relevance inference.
    pub description: String,

    /// Behavior classification.
    pub kind: MemoryKind,

    /// Whether this memory must be read at least once per session.
    #[serde(default)]
    pub mandatory: bool,

    /// Current published version. Managed by the server; ignored on
    /// incoming client edits.
    #[serde(default)]
    pub version: Option<semver::Version>,

    /// Free-form classification tags.
    #[serde(default)]
    pub tags: Vec<String>,

    /// Bump intent hint for the next version assignment. Editors set
    /// this; the server consumes and clears it at push time.
    #[serde(default)]
    pub bump_intent: Option<BumpIntent>,
}
