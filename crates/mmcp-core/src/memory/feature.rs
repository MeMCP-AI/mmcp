//! Typed metadata for feature-request memories.
//!
//! Only memories whose [`MemoryKind`](crate::memory::MemoryKind) is
//! `Fr` carry this block. On non-FR memories the frontmatter
//! serializes without a `[feature]` subtable so the existing rule /
//! snapshot / log / reference / scratch wire shape is unchanged.
//!
//! [`FeatureStatus`] and [`FeatureMetadata`] live in the same file
//! because the struct exists solely to group the status and the two
//! cross-reference lists; splitting them across files would add no
//! clarity.

use serde::{Deserialize, Serialize};

/// Lifecycle state of a feature request.
///
/// Deliberately richer than a boolean `completed` flag so blocked and
/// deferred requests stay visible — they are closed in the sense
/// that no immediate work is expected, but a future sweep may revive
/// them. `Duplicate` captures requests folded into another FR so the
/// cross-reference survives even after the originating slug is
/// superseded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum FeatureStatus {
    /// Active request; not yet resolved, not yet blocked.
    #[default]
    Open,
    /// Work landed. The memory stays in the group for history.
    Resolved,
    /// Waiting on an external prerequisite (another FR, a server
    /// deployment, an upstream design decision).
    Blocked,
    /// Intentionally postponed. Not being worked on, but still valid.
    Deferred,
    /// Superseded by another FR. Body usually carries a pointer to
    /// the surviving slug in the first paragraph.
    Duplicate,
}

impl FeatureStatus {
    /// Canonical lowercase string, matching the serde `snake_case`
    /// serialization. Use for CLI formatting and log output.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            FeatureStatus::Open => "open",
            FeatureStatus::Resolved => "resolved",
            FeatureStatus::Blocked => "blocked",
            FeatureStatus::Deferred => "deferred",
            FeatureStatus::Duplicate => "duplicate",
        }
    }

    /// Every variant in a stable order. Used by list / filter UIs
    /// so the enum stays the single source of truth.
    #[must_use]
    pub const fn all() -> &'static [FeatureStatus] {
        &[
            FeatureStatus::Open,
            FeatureStatus::Resolved,
            FeatureStatus::Blocked,
            FeatureStatus::Deferred,
            FeatureStatus::Duplicate,
        ]
    }

    /// Parse the lowercase wire form back into a variant.
    ///
    /// Errors carry the offending input so callers can surface it in
    /// a user-facing message without juggling `parse::<FeatureStatus>()`
    /// context.
    pub fn parse(raw: &str) -> Result<Self, FeatureStatusParseError> {
        match raw {
            "open" => Ok(FeatureStatus::Open),
            "resolved" => Ok(FeatureStatus::Resolved),
            "blocked" => Ok(FeatureStatus::Blocked),
            "deferred" => Ok(FeatureStatus::Deferred),
            "duplicate" => Ok(FeatureStatus::Duplicate),
            other => Err(FeatureStatusParseError {
                input: other.to_string(),
            }),
        }
    }
}

/// Raised when [`FeatureStatus::parse`] sees a string that does not
/// match any variant. Kept as its own type so CLI + MCP callers can
/// map it onto their respective error shapes.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error(
    "invalid feature status '{input}': expected one of open / resolved / blocked / deferred / duplicate"
)]
pub struct FeatureStatusParseError {
    /// The offending input string, echoed back for user-facing errors.
    pub input: String,
}

/// Structured block describing a feature request, carried inside
/// [`MemoryFrontmatter::feature`](crate::memory::MemoryFrontmatter)
/// when — and only when — the memory's `kind` is `Fr`.
///
/// Absent block (serialized as no `[feature]` subtable) is equivalent
/// to `FeatureMetadata::default()` for kind=Fr memories written by
/// older clients; newer writers always emit the block explicitly.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FeatureMetadata {
    /// Current lifecycle state.
    #[serde(default)]
    pub status: FeatureStatus,

    /// Slugs of FRs this one depends on; usually the prerequisite
    /// surface must land first. Rendered as a list in diagnostics so
    /// cycles surface visibly.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub depends_on: Vec<String>,

    /// Slugs of FRs whose own resolution is gated on this one. The
    /// inverse of `depends_on` maintained explicitly so neither
    /// direction of the graph needs a scan to enumerate.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blocks: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_round_trips_through_str_and_parse() {
        for variant in FeatureStatus::all() {
            let parsed = FeatureStatus::parse(variant.as_str()).expect("round trip");
            assert_eq!(&parsed, variant);
        }
    }

    #[test]
    fn status_parse_rejects_unknown() {
        let err = FeatureStatus::parse("wontfix").expect_err("unknown status must fail");
        assert_eq!(err.input, "wontfix");
    }

    #[test]
    fn default_metadata_is_open_with_empty_cross_refs() {
        let meta = FeatureMetadata::default();
        assert_eq!(meta.status, FeatureStatus::Open);
        assert!(meta.depends_on.is_empty());
        assert!(meta.blocks.is_empty());
    }
}
