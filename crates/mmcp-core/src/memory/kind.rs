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
    /// frontmatter (status, depends_on, blocks) so the feature
    /// lifecycle tools can filter and cross-reference without
    /// parsing the body.
    ///
    /// The `fr` serde alias keeps pre-FR-027 memories readable;
    /// writers emit `feature` so a rewrite on the next edit
    /// auto-upgrades the on-disk wire form.
    #[serde(alias = "fr")]
    Feature,

    /// Issue tracker entry. Sister kind to `Feature`. Carries a
    /// structured [`IssueMetadata`](crate::memory::IssueMetadata)
    /// block in frontmatter (status, depends_on, blocks). The
    /// hybrid model permits a memory to carry both `[feature]`
    /// and `[issue]` blocks; listings filter by block presence.
    Issue,
}

impl MemoryKind {
    /// The canonical string representation of this kind, matching
    /// the serde `snake_case` serialization. FR-027 renamed the
    /// former `Fr` variant to `Feature`; the wire emits `feature`
    /// while the `fr` serde alias preserves read compatibility.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            MemoryKind::Rule => "rule",
            MemoryKind::Snapshot => "snapshot",
            MemoryKind::Log => "log",
            MemoryKind::Reference => "reference",
            MemoryKind::Scratch => "scratch",
            MemoryKind::Feature => "feature",
            MemoryKind::Issue => "issue",
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn feature_kind_serializes_as_feature() {
        let json = serde_json::to_string(&MemoryKind::Feature).expect("serialize");
        assert_eq!(json, "\"feature\"");
    }

    #[test]
    fn feature_kind_accepts_fr_serde_alias_for_read_compat() {
        // Pre-FR-027 memories on disk carry `kind = "fr"`; the
        // alias keeps them readable without a migration so the
        // rename can land before the disk rewrite.
        let parsed: MemoryKind = serde_json::from_str("\"fr\"").expect("fr alias");
        assert_eq!(parsed, MemoryKind::Feature);
    }

    #[test]
    fn feature_kind_also_reads_canonical_feature_string() {
        let parsed: MemoryKind = serde_json::from_str("\"feature\"").expect("feature");
        assert_eq!(parsed, MemoryKind::Feature);
    }

    #[test]
    fn as_str_returns_canonical_feature_token() {
        assert_eq!(MemoryKind::Feature.as_str(), "feature");
    }
}
