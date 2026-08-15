//! Classification of a memory's behavior at retrieval time.

use serde::{Deserialize, Serialize};

/// Declares the full `MemoryKind` variant list exactly once.
///
/// Every mirror of that list (the enum itself, `as_str()`,
/// `FromStr`, [`MemoryKind::ALL`], and the parse-error message text)
/// expands from this single invocation, so a new variant can never
/// update one mirror while leaving another behind. See the
/// invocation below for the actual variant list and doc comments.
macro_rules! define_memory_kind {
    (
        $(
            $(#[$variant_meta:meta])*
            $variant:ident => $wire:literal
        ),+ $(,)?
    ) => {
        /// Built-in memory kinds shipped with mmcp.
        ///
        /// Each kind carries behavioral implications, not just classification.
        /// The retrieval layer inspects the kind to decide whether to attach staleness warnings,
        /// whether edits are restricted to appends, and whether versioning applies.
        ///
        /// Users may register custom kinds on top of these defaults,
        /// but the core set is fixed because server-side logic branches on it.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(rename_all = "snake_case")]
        pub enum MemoryKind {
            $(
                $(#[$variant_meta])*
                $variant,
            )+
        }

        impl MemoryKind {
            /// Every variant, in declaration order.
            ///
            /// The single generated-list consumer: exhaustive tests
            /// (round-trip, TS-export drift) iterate this instead of
            /// hand-listing variants a second time.
            pub const ALL: &'static [MemoryKind] = &[$(MemoryKind::$variant),+];

            /// The canonical string representation of this kind, matching the serde `snake_case` serialization.
            /// `feature` is the only wire form accepted for this variant.
            #[must_use]
            pub const fn as_str(self) -> &'static str {
                match self {
                    $(MemoryKind::$variant => $wire,)+
                }
            }
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
                    $($wire => Ok(MemoryKind::$variant),)+
                    other => Err(MemoryKindParseError {
                        input: other.to_string(),
                    }),
                }
            }
        }
    };
}

define_memory_kind! {
    /// Stable convention or guideline.
    /// Session-agnostic, no automatic staleness warning attached.
    Rule => "rule",

    /// Point-in-time fact about the project (status, counts, test results).
    /// Retrieval always attaches a "may be stale" warning because snapshots rot by definition.
    Snapshot => "snapshot",

    /// Append-only record (decisions, incidents).
    /// Edits may only add entries; prior entries are immutable.
    Log => "log",

    /// Pointer to an external resource (Linear project, Grafana dashboard, spec URL).
    /// Rarely changes; no staleness warning.
    Reference => "reference",

    /// Short-lived working notes.
    /// Not versioned, no warnings.
    Scratch => "scratch",

    /// Feature request.
    /// Carries a structured [`FeatureMetadata`](crate::memory::FeatureMetadata)
    /// block in frontmatter (status, depends_on, blocks)
    /// so the feature lifecycle tools can filter and cross-reference without parsing the body.
    Feature => "feature",

    /// Issue tracker entry.
    /// Sister kind to `Feature`.
    /// Carries a structured [`IssueMetadata`](crate::memory::IssueMetadata)
    /// block in frontmatter (status, depends_on, blocks).
    /// The hybrid model permits a memory to carry both `[feature]` and `[issue]` blocks;
    /// listings filter by block presence.
    Issue => "issue",

    /// Milestone: a grouping container over features, possibly
    /// spanning multiple project groups.
    /// Carries a structured [`MilestoneMetadata`](crate::memory::MilestoneMetadata) block in frontmatter.
    /// Deliberately a reduced-surface tracked kind: no supersede flow,
    /// no `depends_on` / `blocks`, no shared ticket-number counter.
    /// Individual features opt into a milestone
    /// via [`FeatureMetadata::milestone`](crate::memory::FeatureMetadata::milestone);
    /// the milestone's own live status is computed by `mmcp_store::rollup`,
    /// never stored here.
    Milestone => "milestone",
}

impl MemoryKind {
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
///
/// `Display` is hand-written rather than a `thiserror` `#[error(...)]`
/// attribute because the message body must join
/// [`MemoryKind::ALL`]'s wire strings at runtime; a `thiserror`
/// attribute literal is fixed at macro-expansion time and cannot
/// interpolate that runtime-built join, which is exactly the
/// hand-listed mirror this restructuring closes. `input` and the
/// `FromStr::Err` association are unchanged public API.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryKindParseError {
    /// The offending input string, echoed back for user-facing errors.
    pub input: String,
}

impl std::fmt::Display for MemoryKindParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "invalid memory kind '{}': expected one of ", self.input)?;
        for (index, kind) in MemoryKind::ALL.iter().enumerate() {
            if index > 0 {
                write!(f, " / ")?;
            }
            write!(f, "{}", kind.as_str())?;
        }
        Ok(())
    }
}

impl std::error::Error for MemoryKindParseError {}

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
        // "fr" is not a recognized kind value; only "feature" parses.
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
        for kind in MemoryKind::ALL {
            let parsed: MemoryKind = kind.as_str().parse().expect("round trip");
            assert_eq!(parsed, *kind);
        }
    }

    #[test]
    fn from_str_rejects_unknown_kind() {
        let err = "bogus".parse::<MemoryKind>().expect_err("unknown kind");
        assert_eq!(err.input, "bogus");
    }

    #[test]
    fn parse_error_message_lists_every_kind() {
        let err = "bogus".parse::<MemoryKind>().expect_err("unknown kind");
        assert_eq!(
            err.to_string(),
            "invalid memory kind 'bogus': expected one of rule / snapshot / log / reference / scratch / feature / issue / milestone"
        );
    }
}
