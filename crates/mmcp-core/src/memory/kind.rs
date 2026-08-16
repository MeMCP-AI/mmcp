//! Classification of a memory's behavior at retrieval time.

use serde::{Deserialize, Serialize};
use strum::VariantArray as _;
use strum_macros::{IntoStaticStr, VariantArray};

/// Built-in memory kinds shipped with mmcp.
///
/// Each kind carries behavioral implications, not just classification.
/// The retrieval layer inspects the kind to decide whether to attach staleness warnings,
/// whether edits are restricted to appends, and whether versioning applies.
///
/// Users may register custom kinds on top of these defaults,
/// but the core set is fixed because server-side logic branches on it.
///
/// [`VariantArray`] derives [`Self::VARIANTS`] (every variant, in
/// declaration order) and [`IntoStaticStr`] derives the wire-name
/// conversion (`self.into(): &'static str`). `FromStr` and
/// [`MemoryKindParseError`] stay hand-written below: the error message
/// joins [`MemoryKind::VARIANTS`]'s wire strings at runtime, which
/// strum's derived `EnumString`/parse-error type cannot reproduce.
/// `#[strum(serialize_all = "snake_case")]` matches the `#[serde(rename_all
/// = "snake_case")]` wire form below, so both mirrors agree by
/// construction.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, VariantArray, IntoStaticStr,
)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum MemoryKind {
    /// Stable convention or guideline.
    /// Session-agnostic, no automatic staleness warning attached.
    Rule,

    /// Point-in-time fact about the project (status, counts, test results).
    /// Retrieval always attaches a "may be stale" warning because snapshots rot by definition.
    Snapshot,

    /// Append-only record (decisions, incidents).
    /// Edits may only add entries; prior entries are immutable.
    Log,

    /// Pointer to an external resource (Linear project, Grafana dashboard, spec URL).
    /// Rarely changes; no staleness warning.
    Reference,

    /// Short-lived working notes.
    /// Not versioned, no warnings.
    Scratch,

    /// Feature request.
    /// Carries a structured [`FeatureMetadata`](crate::memory::FeatureMetadata)
    /// block in frontmatter (status, depends_on, blocks)
    /// so the feature lifecycle tools can filter and cross-reference without parsing the body.
    Feature,

    /// Issue tracker entry.
    /// Sister kind to `Feature`.
    /// Carries a structured [`IssueMetadata`](crate::memory::IssueMetadata)
    /// block in frontmatter (status, depends_on, blocks).
    /// The hybrid model permits a memory to carry both `[feature]` and `[issue]` blocks;
    /// listings filter by block presence.
    Issue,

    /// Milestone: a grouping container over features, possibly
    /// spanning multiple project groups.
    /// Carries a structured [`MilestoneMetadata`](crate::memory::MilestoneMetadata) block in frontmatter.
    /// Deliberately a reduced-surface tracked kind: no supersede flow,
    /// no `depends_on` / `blocks`, no shared ticket-number counter.
    /// Individual features opt into a milestone
    /// via [`FeatureMetadata::milestone`](crate::memory::FeatureMetadata::milestone);
    /// the milestone's own live status is computed by `mmcp_store::rollup`,
    /// never stored here.
    Milestone,
}

impl MemoryKind {
    /// The canonical string representation of this kind, matching the serde `snake_case` serialization.
    /// `feature` is the only wire form accepted for this variant.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        self.into()
    }
}

/// Canonical parser for the lowercase wire form of [`MemoryKind`].
///
/// This is the single owning parser every hand-rolled `MemoryKind`
/// decoder in the codebase delegates to (mmcp-store's create-time
/// parser, the archive filter's facet parser, the GUI's DTO
/// converter) so the accepted-kind set can never drift between
/// call sites again.
///
/// Hand-written rather than strum's derived `FromStr`/`EnumString`:
/// [`MemoryKindParseError`]'s `Display` joins [`MemoryKind::VARIANTS`]'s
/// wire strings at runtime, which strum's derived error type cannot
/// reproduce (see that type's doc comment).
impl std::str::FromStr for MemoryKind {
    type Err = MemoryKindParseError;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        MemoryKind::VARIANTS
            .iter()
            .copied()
            .find(|kind| kind.as_str() == raw)
            .ok_or_else(|| MemoryKindParseError {
                input: raw.to_string(),
            })
    }
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
/// [`MemoryKind::VARIANTS`]'s wire strings at runtime; a `thiserror`
/// attribute literal is fixed at macro-expansion time and cannot
/// interpolate that runtime-built join.
/// `input` and the `FromStr::Err` association are unchanged public API.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryKindParseError {
    /// The offending input string, echoed back for user-facing errors.
    pub input: String,
}

impl std::fmt::Display for MemoryKindParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "invalid memory kind '{}': expected one of ", self.input)?;
        for (index, kind) in MemoryKind::VARIANTS.iter().enumerate() {
            if index > 0 {
                write!(f, " / ")?;
            }
            write!(f, "{}", kind.as_str())?;
        }
        Ok(())
    }
}

impl std::error::Error for MemoryKindParseError {}

/// Environment variable that switches the TS-drift test below from
/// asserting to regenerating: `MMCP_UPDATE_GENERATED=1 cargo test -p
/// mmcp-core` rewrites `memory_kind.generated.ts` from
/// [`MemoryKind::VARIANTS`] instead of comparing against it.
#[cfg(test)]
const GENERATED_TS_UPDATE_ENV: &str = "MMCP_UPDATE_GENERATED";

/// Path to the generated TypeScript array, relative to this crate's
/// manifest directory (`crates/mmcp-core`): two levels up reaches
/// the repo root, then down into the GUI's utils folder.
#[cfg(test)]
const GENERATED_TS_RELATIVE_PATH: &str = "../../gui/src/lib/utils/memory_kind.generated.ts";

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
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
        for kind in MemoryKind::VARIANTS {
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

    /// Renders the exact TypeScript source
    /// `memory_kind.generated.ts` must contain for the current
    /// [`MemoryKind::VARIANTS`].
    fn render_generated_ts() -> String {
        let values = MemoryKind::VARIANTS
            .iter()
            .map(|kind| format!("'{}'", kind.as_str()))
            .collect::<Vec<_>>()
            .join(", ");
        format!(
            "// GENERATED FILE - DO NOT EDIT BY HAND.\n\
             // Source of truth: crates/mmcp-core/src/memory/kind.rs (MemoryKind).\n\
             // Regenerate: {GENERATED_TS_UPDATE_ENV}=1 cargo test -p mmcp-core\n\
             export const MEMORY_KIND_VALUES = [\n  {values}\n] as const;\n"
        )
    }

    /// Normalizes CRLF to LF so a Windows checkout's line endings
    /// never register as a content drift: this repo carries no
    /// `.gitattributes` enforcement beyond the generated file itself,
    /// so a CRLF checkout of the generated TS file is a real risk.
    fn normalize_line_endings(text: &str) -> String {
        text.replace("\r\n", "\n")
    }

    /// Drift guard between [`MemoryKind::VARIANTS`] and the generated
    /// `memory_kind.generated.ts` the GUI imports at
    /// `gui/src/lib/utils/memory_kind.ts`.
    ///
    /// Set `MMCP_UPDATE_GENERATED=1` to rewrite the file from the
    /// current `MemoryKind::VARIANTS` instead of asserting against it.
    #[test]
    fn generated_ts_matches_memory_kind_all() {
        let expected = render_generated_ts();
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(GENERATED_TS_RELATIVE_PATH);

        if std::env::var(GENERATED_TS_UPDATE_ENV).is_ok() {
            std::fs::write(&path, &expected)
                .unwrap_or_else(|error| panic!("failed to write {}: {error}", path.display()));
            return;
        }

        let actual = std::fs::read_to_string(&path).unwrap_or_else(|error| {
            panic!(
                "failed to read {}: {error}; regenerate via `{GENERATED_TS_UPDATE_ENV}=1 cargo test -p mmcp-core generated_ts_matches_memory_kind_all`",
                path.display()
            )
        });

        assert_eq!(
            normalize_line_endings(&actual),
            normalize_line_endings(&expected),
            "gui/src/lib/utils/memory_kind.generated.ts is out of sync with MemoryKind::VARIANTS; regenerate via `{GENERATED_TS_UPDATE_ENV}=1 cargo test -p mmcp-core generated_ts_matches_memory_kind_all`"
        );
    }
}
