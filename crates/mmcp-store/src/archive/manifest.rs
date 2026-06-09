//! The `archive.toml` table of contents at an archive's root.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Name of the manifest entry at the archive root.
pub const ARCHIVE_MANIFEST_FILENAME: &str = "archive.toml";

/// Directory each group's contents live under inside the archive.
pub const ARCHIVE_GROUPS_DIR: &str = "groups";

/// Archive layout version. Bumped only on a breaking layout change;
/// a reader rejects any archive whose `format_version` exceeds the
/// version it was built with.
pub const ARCHIVE_FORMAT_VERSION: u32 = 1;

/// What an archive carries for each group.
///
/// One variant today. The mode is kept as an explicit field rather
/// than implied so a future full-git-history mode lands as an
/// additive variant without reshaping the manifest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ArchiveMode {
    /// HEAD snapshot: every memory file verbatim at its current
    /// revision, no version history.
    #[default]
    Snapshot,
}

/// One group's summary row in the archive table of contents.
///
/// The authoritative group metadata is the verbatim `.mmcp.toml`
/// stored alongside the group's memories; this row is a fast index
/// for listing and validation without unpacking the whole stream.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArchivedGroupMeta {
    /// Group UUID, matching the `groups/<group-uuid>/` directory.
    pub group_id: Uuid,
    /// Group slug at export time.
    pub slug: String,
    /// Optional human-facing group name at export time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    /// Number of memory files archived for this group.
    pub memory_count: u32,
}

/// Root descriptor (`archive.toml`) of an mmcp archive.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArchiveManifest {
    /// Archive layout version; see [`ARCHIVE_FORMAT_VERSION`].
    pub format_version: u32,
    /// What the archive carries (snapshot vs, later, history).
    pub mode: ArchiveMode,
    /// mmcp version that produced the archive.
    pub mmcp_version: String,
    /// RFC 3339 creation timestamp.
    pub created_at: String,
    /// One row per group included in the archive.
    pub groups: Vec<ArchivedGroupMeta>,
}

impl ArchiveManifest {
    /// Render to the TOML stored at the archive root.
    pub fn to_toml(&self) -> Result<String, toml::ser::Error> {
        toml::to_string_pretty(self)
    }

    /// Parse the TOML read from the archive root.
    pub fn from_toml(source: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(source)
    }

    /// Total memory files across every archived group.
    #[must_use]
    pub fn total_memory_count(&self) -> u64 {
        self.groups.iter().map(|g| u64::from(g.memory_count)).sum()
    }
}
