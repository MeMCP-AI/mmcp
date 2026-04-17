//! Classification of a memory's behavior at retrieval time.

use serde::{Deserialize, Serialize};

/// Built-in memory kinds shipped with mmcp.
///
/// Each kind carries behavioral implications, not just classification.
/// The retrieval layer inspects the kind to decide whether to attach
/// staleness warnings, whether edits are restricted to appends, and
/// whether versioning applies.
///
/// Users may register custom kinds on top of these defaults, but the
/// core set is fixed because server-side logic branches on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryKind {
    /// Stable convention or guideline. Session-agnostic, no automatic
    /// staleness warning attached.
    Rule,

    /// Point-in-time fact about the project (status, counts, test
    /// results). Retrieval always attaches a "may be stale" warning
    /// because snapshots rot by definition.
    Snapshot,

    /// Append-only record (decisions, incidents). Edits may only add
    /// entries; prior entries are immutable.
    Log,

    /// Pointer to an external resource (Linear project, Grafana
    /// dashboard, spec URL). Rarely changes; no staleness warning.
    Reference,

    /// Short-lived working notes. Not versioned, no warnings.
    Scratch,

    /// Feature request. Carries a structured
    /// [`FeatureMetadata`](crate::memory::FeatureMetadata) block in
    /// frontmatter (status, depends_on, blocks) so the FR lifecycle
    /// tools can filter and cross-reference without parsing the body.
    Fr,
}

impl MemoryKind {
    /// The canonical string representation of this kind, matching
    /// the serde `snake_case` serialization.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            MemoryKind::Rule => "rule",
            MemoryKind::Snapshot => "snapshot",
            MemoryKind::Log => "log",
            MemoryKind::Reference => "reference",
            MemoryKind::Scratch => "scratch",
            MemoryKind::Fr => "fr",
        }
    }

    /// True if retrieval should attach a "content may be out of date"
    /// warning by default for this kind.
    #[must_use]
    pub const fn warns_stale(self) -> bool {
        matches!(self, MemoryKind::Snapshot)
    }

    /// True if edits to this kind must be append-only.
    #[must_use]
    pub const fn is_append_only(self) -> bool {
        matches!(self, MemoryKind::Log)
    }

    /// True if this kind participates in the semver versioning system.
    ///
    /// Scratch memories are intentionally excluded so short-lived
    /// notes do not accumulate version history.
    #[must_use]
    pub const fn is_versioned(self) -> bool {
        !matches!(self, MemoryKind::Scratch)
    }
}
