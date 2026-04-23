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
use uuid::Uuid;

use crate::memory::MemoryRef;

/// Lifecycle state of a feature request.
///
/// Deliberately richer than a boolean `completed` flag so blocked and
/// deferred requests stay visible — they are closed in the sense
/// that no immediate work is expected, but a future sweep may revive
/// them.
///
/// `Duplicate` and `Superseded` sound similar but mean different
/// things. `Duplicate` = "this was filed twice by accident, see the
/// surviving slug in the body". `Superseded` = "this idea evolved;
/// a newer, better-scoped FR replaces it and the supersede flow
/// recorded a typed back-link in `FeatureMetadata::superseded_by`".
/// The two statuses are kept distinct so listings can surface the
/// supersede lineage without conflating it with accidental dupes.
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
    /// Duplicate of another FR. Body usually carries a pointer to
    /// the surviving slug in the first paragraph.
    Duplicate,
    /// Replaced by a newer FR via the typed supersede flow.
    /// [`FeatureMetadata::superseded_by`] points at the replacement.
    Superseded,
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
            FeatureStatus::Superseded => "superseded",
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
            FeatureStatus::Superseded,
        ]
    }

    /// Statuses hidden from `list_features(all = false)` when no
    /// explicit status filter is set. The three variants share one
    /// property: no further action is expected without an operator
    /// explicitly pulling them back into the listing, so noisy
    /// default listings stay actionable.
    ///
    /// Kept on the enum rather than hardcoded at call sites so
    /// future variants declare their default visibility once.
    #[must_use]
    pub const fn is_default_hidden(self) -> bool {
        matches!(
            self,
            FeatureStatus::Resolved | FeatureStatus::Duplicate | FeatureStatus::Superseded,
        )
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
            "superseded" => Ok(FeatureStatus::Superseded),
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
    "invalid feature status '{input}': expected one of open / resolved / blocked / deferred / duplicate / superseded"
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

    /// Sequential number per group, auto-assigned by `add_feature`
    /// as `max(existing_numbers) + 1`. Gaps from deletions are not
    /// reused so the lineage stays monotonic. Absent on pre-FR-027
    /// memories until the slug-migration binary backfills them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub number: Option<u32>,

    /// UUIDs of FRs this one depends on; usually the prerequisite
    /// surface must land first. Post-FR-028 cross-refs hold memory
    /// UUIDs (not slugs) so a rename on either side never breaks
    /// the graph. Rendered as a list in diagnostics so cycles
    /// surface visibly.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub depends_on: Vec<Uuid>,

    /// UUIDs of FRs whose own resolution is gated on this one. The
    /// inverse of `depends_on` maintained explicitly so neither
    /// direction of the graph needs a scan to enumerate.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blocks: Vec<Uuid>,

    /// Typed back-link to the FR that replaced this one, set by the
    /// supersede flow on `add_feature`. Paired with
    /// [`FeatureStatus::Superseded`] by the
    /// [`FeatureMetadata::validate_supersede_invariant`] check: when
    /// this field is `Some`, status must be `Superseded`, and vice
    /// versa.
    ///
    /// The commit sha inside the [`MemoryRef`] pins the replacement
    /// to the exact revision that marked this FR superseded, so
    /// later edits on the replacement never silently change the
    /// lineage a reader sees when they follow the link.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub superseded_by: Option<MemoryRef>,
}

impl FeatureMetadata {
    /// Enforce the paired invariant between `status` and
    /// `superseded_by`: either both signal supersession, or neither
    /// does. Called from write paths that construct or mutate a
    /// `FeatureMetadata` so partial states never reach the on-disk
    /// frontmatter.
    ///
    /// Returns the offending combination in the error variant so
    /// CLI and MCP error mappers can surface it without
    /// re-inspecting the struct.
    pub fn validate_supersede_invariant(&self) -> Result<(), SupersedeInvariantError> {
        match (self.status, self.superseded_by.is_some()) {
            (FeatureStatus::Superseded, true) => Ok(()),
            (FeatureStatus::Superseded, false) => {
                Err(SupersedeInvariantError::MissingSupersededBy)
            }
            (other, true) => Err(SupersedeInvariantError::UnexpectedSupersededBy {
                status: other,
            }),
            (_, false) => Ok(()),
        }
    }
}

/// Raised by [`FeatureMetadata::validate_supersede_invariant`] when
/// `status` and `superseded_by` disagree on whether the FR is
/// superseded.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SupersedeInvariantError {
    /// `status == Superseded` but no `superseded_by` link is set.
    /// Callers should either fill in the typed back-link or move
    /// the FR into a different closed status.
    #[error("status is superseded but superseded_by is empty")]
    MissingSupersededBy,
    /// `superseded_by` is set but `status` is not `Superseded`.
    /// Callers should either clear the back-link or flip the
    /// status.
    #[error("superseded_by is set but status is {status:?}, expected superseded")]
    UnexpectedSupersededBy { status: FeatureStatus },
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
        assert_eq!(meta.number, None);
        assert!(meta.superseded_by.is_none());
    }

    #[test]
    fn number_field_round_trips_through_toml() {
        let meta = FeatureMetadata {
            status: FeatureStatus::Open,
            number: Some(42),
            depends_on: Vec::new(),
            blocks: Vec::new(),
            superseded_by: None,
        };
        let rendered = toml::to_string(&meta).expect("render");
        assert!(rendered.contains("number = 42"), "rendered: {rendered}");
        let parsed: FeatureMetadata = toml::from_str(&rendered).expect("parse");
        assert_eq!(parsed.number, Some(42));
    }

    #[test]
    fn absent_number_is_skipped_on_serialize() {
        let meta = FeatureMetadata::default();
        let rendered = toml::to_string(&meta).expect("render");
        assert!(!rendered.contains("number"), "rendered: {rendered}");
    }

    fn forty_char_hex() -> &'static str {
        "0123456789abcdef0123456789abcdef01234567"
    }

    #[test]
    fn is_default_hidden_covers_resolved_duplicate_superseded() {
        for variant in FeatureStatus::all() {
            let hidden = variant.is_default_hidden();
            match variant {
                FeatureStatus::Resolved
                | FeatureStatus::Duplicate
                | FeatureStatus::Superseded => assert!(hidden, "{variant:?} must be default-hidden"),
                FeatureStatus::Open | FeatureStatus::Blocked | FeatureStatus::Deferred => {
                    assert!(!hidden, "{variant:?} must stay visible by default")
                }
            }
        }
    }

    #[test]
    fn superseded_variant_round_trips_through_str_and_parse() {
        assert_eq!(FeatureStatus::Superseded.as_str(), "superseded");
        let parsed = FeatureStatus::parse("superseded").expect("round trip");
        assert_eq!(parsed, FeatureStatus::Superseded);
    }

    #[test]
    fn parse_error_message_mentions_superseded() {
        let err = FeatureStatus::parse("wontfix").expect_err("unknown must fail");
        assert!(err.to_string().contains("superseded"), "message: {err}");
    }

    #[test]
    fn supersede_invariant_accepts_both_set() {
        let meta = FeatureMetadata {
            status: FeatureStatus::Superseded,
            number: None,
            depends_on: Vec::new(),
            blocks: Vec::new(),
            superseded_by: Some(MemoryRef::new(Uuid::now_v7(), forty_char_hex())),
        };
        assert!(meta.validate_supersede_invariant().is_ok());
    }

    #[test]
    fn supersede_invariant_accepts_neither_set() {
        let meta = FeatureMetadata::default();
        assert!(meta.validate_supersede_invariant().is_ok());
    }

    #[test]
    fn supersede_invariant_rejects_status_without_link() {
        let mut meta = FeatureMetadata::default();
        meta.status = FeatureStatus::Superseded;
        let err = meta
            .validate_supersede_invariant()
            .expect_err("status without link must fail");
        assert_eq!(err, SupersedeInvariantError::MissingSupersededBy);
    }

    #[test]
    fn supersede_invariant_rejects_link_without_status() {
        let mut meta = FeatureMetadata::default();
        meta.superseded_by = Some(MemoryRef::new(Uuid::now_v7(), forty_char_hex()));
        let err = meta
            .validate_supersede_invariant()
            .expect_err("link without status must fail");
        assert_eq!(
            err,
            SupersedeInvariantError::UnexpectedSupersededBy {
                status: FeatureStatus::Open,
            }
        );
    }

    #[test]
    fn superseded_by_round_trips_through_toml() {
        let id = Uuid::now_v7();
        let meta = FeatureMetadata {
            status: FeatureStatus::Superseded,
            number: Some(12),
            depends_on: Vec::new(),
            blocks: Vec::new(),
            superseded_by: Some(MemoryRef::new(id, forty_char_hex())),
        };
        let rendered = toml::to_string(&meta).expect("render");
        assert!(rendered.contains("status = \"superseded\""), "rendered: {rendered}");
        assert!(rendered.contains("[superseded_by]"), "rendered: {rendered}");
        let parsed: FeatureMetadata = toml::from_str(&rendered).expect("parse");
        assert_eq!(parsed, meta);
    }
}
