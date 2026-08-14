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
    Feature,

    /// Issue tracker entry. Sister kind to `Feature`. Carries a
    /// structured [`IssueMetadata`](crate::memory::IssueMetadata)
    /// block in frontmatter (status, depends_on, blocks). The
    /// hybrid model permits a memory to carry both `[feature]`
    /// and `[issue]` blocks; listings filter by block presence.
    Issue,

    /// Milestone: a grouping container over features, possibly
    /// spanning multiple project groups. Carries a structured
    /// [`MilestoneMetadata`](crate::memory::MilestoneMetadata)
    /// block in frontmatter. Deliberately a reduced-surface tracked
    /// kind: no supersede flow, no `depends_on` /
    /// `blocks`, no shared ticket-number counter. Individual
    /// features opt into a milestone via
    /// [`FeatureMetadata::milestone`](crate::memory::FeatureMetadata::milestone);
    /// the milestone's own live status is computed by
    /// `mmcp_store::rollup`, never stored here.
    Milestone,
}

impl MemoryKind {
    /// The canonical string representation of this kind, matching the serde `snake_case` serialization.
    /// `feature` is the only wire form accepted for this variant.
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
            MemoryKind::Milestone => "milestone",
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

/// Raised when [`MemoryKind`]'s [`FromStr`](std::str::FromStr) impl
/// sees a string that does not match any known kind.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error(
    "invalid memory kind '{input}': expected one of rule / snapshot / log / reference / scratch / feature / issue / milestone"
)]
pub struct MemoryKindParseError {
    /// The offending input string, echoed back for user-facing errors.
    pub input: String,
}

/// Canonical parser for the lowercase wire form of [`MemoryKind`].
///
/// This is the single owning parser every hand-rolled `MemoryKind`
/// decoder in the codebase delegates to (mmcp-store's create-time
/// parser, the archive filter's facet parser, the GUI's DTO
/// converter) so the accepted-kind set can never drift between
/// call sites again.
impl std::str::FromStr for MemoryKind {
    type Err = MemoryKindParseError;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        match raw {
            "rule" => Ok(MemoryKind::Rule),
            "snapshot" => Ok(MemoryKind::Snapshot),
            "log" => Ok(MemoryKind::Log),
            "reference" => Ok(MemoryKind::Reference),
            "scratch" => Ok(MemoryKind::Scratch),
            "feature" => Ok(MemoryKind::Feature),
            "issue" => Ok(MemoryKind::Issue),
            "milestone" => Ok(MemoryKind::Milestone),
            other => Err(MemoryKindParseError {
                input: other.to_string(),
            }),
        }
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
    fn feature_kind_rejects_legacy_fr_alias() {
        // The `fr` serde alias was retired: `migrate_fr_slugs` ran
        // months ago and rewrote every on-disk memory to the
        // canonical `feature` spelling, so legacy code is code to
        // delete. A `kind = "fr"` memory must now fail to parse
        // rather than silently round-tripping through a dead alias.
        let result: Result<MemoryKind, _> = serde_json::from_str("\"fr\"");
        assert!(result.is_err());
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

    #[test]
    fn from_str_round_trips_every_kind() {
        for kind in [
            MemoryKind::Rule,
            MemoryKind::Snapshot,
            MemoryKind::Log,
            MemoryKind::Reference,
            MemoryKind::Scratch,
            MemoryKind::Feature,
            MemoryKind::Issue,
            MemoryKind::Milestone,
        ] {
            let parsed: MemoryKind = kind.as_str().parse().expect("round trip");
            assert_eq!(parsed, kind);
        }
    }

    #[test]
    fn from_str_rejects_unknown_kind() {
        let err = "bogus".parse::<MemoryKind>().expect_err("unknown kind");
        assert_eq!(err.input, "bogus");
    }
}
