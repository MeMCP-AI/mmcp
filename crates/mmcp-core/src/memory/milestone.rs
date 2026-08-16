//! Typed metadata for milestone memories.
//!
//! Sister surface to [`feature`](super::feature) and
//! [`issue`](super::issue), but deliberately a REDUCED-surface
//! tracked kind: a milestone carries no supersede flow,
//! no `depends_on` / `blocks` cross-refs, and no shared ticket-number
//! counter. A milestone does not need any of that machinery: it is
//! a grouping container, not an individually-worked ticket.
//!
//! Unlike [`FeatureStatus`](super::FeatureStatus) /
//! [`IssueStatus`](super::IssueStatus), [`MilestoneStatus`] is an
//! EDITORIAL field the operator sets directly (`add_milestone` /
//! `update_milestone`), not a fold over anything. The milestone's
//! LIVE, computed status (the fold over the lifecycle states of
//! every feature pointing at it via
//! [`FeatureMetadata::milestone`](super::FeatureMetadata::milestone))
//! is a distinct, separately-named concept computed by
//! `mmcp_store::rollup` at read time and never persisted in
//! frontmatter. Keeping the two separate lets `diagnose` flag the
//! case where they disagree (the operator marked a milestone
//! `completed` but the rollup says otherwise, or vice versa) as a
//! genuine "stale milestone status" finding instead of always
//! trusting one or the other.
//!
//! [`MilestoneStatus`] and [`MilestoneMetadata`] live in the same
//! file because the struct exists solely to group the one status
//! field; splitting them across files would add no clarity.

use serde::{Deserialize, Serialize};

use crate::memory::Status;

/// Editorial lifecycle state of a milestone, set directly by the
/// operator rather than computed.
///
/// Four states, intentionally coarser than [`FeatureStatus`](super::FeatureStatus):
/// a milestone is a grouping container, not a unit of work, so it
/// has no `Blocked` / `Deferred` / `Duplicate` / `Superseded`
/// equivalent of its own: those nuances live on the individual
/// features that make it up and surface through the computed
/// rollup instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum MilestoneStatus {
    /// Freshly filed; still being scoped, features may not yet be
    /// linked.
    #[default]
    Planning,
    /// Actively being worked toward.
    Active,
    /// Explicitly paused by the operator. Distinct from a computed
    /// "blocked" rollup: this is a deliberate editorial pause, not
    /// a fold over feature states.
    OnHold,
    /// Declared done by the operator. The memory stays in the
    /// group for history.
    Completed,
}

impl MilestoneStatus {
    /// Canonical lowercase string, matching the serde `snake_case`
    /// serialization.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            MilestoneStatus::Planning => "planning",
            MilestoneStatus::Active => "active",
            MilestoneStatus::OnHold => "on_hold",
            MilestoneStatus::Completed => "completed",
        }
    }

    /// Every variant in declaration order.
    #[must_use]
    pub const fn all() -> &'static [MilestoneStatus] {
        &[
            MilestoneStatus::Planning,
            MilestoneStatus::Active,
            MilestoneStatus::OnHold,
            MilestoneStatus::Completed,
        ]
    }

    /// Statuses the generic tracker helper
    /// (`mmcp_store::tracker::listing_keeps_status`) treats as
    /// hidden from a default, un-filtered listing. Only `Completed`
    /// counts as terminal-ish here; a milestone on hold still needs
    /// an operator's eyes.
    ///
    /// `list_milestones` (`mmcp_store::milestones`) does NOT go
    /// through that generic helper: it hides based on the
    /// freshly-computed rollup status instead, because this
    /// editorial status can be stale relative to the live rollup
    /// (see the staleness check surfaced by `read_milestone`). This
    /// method's only current callers are the `Status` trait forward
    /// below and that tracker helper, itself unused by milestone
    /// listing.
    #[must_use]
    pub const fn is_default_hidden(self) -> bool {
        matches!(self, MilestoneStatus::Completed)
    }

    /// Parse the lowercase wire form back into a variant.
    pub fn parse(raw: &str) -> Result<Self, MilestoneStatusParseError> {
        match raw {
            "planning" => Ok(MilestoneStatus::Planning),
            "active" => Ok(MilestoneStatus::Active),
            "on_hold" => Ok(MilestoneStatus::OnHold),
            "completed" => Ok(MilestoneStatus::Completed),
            other => Err(MilestoneStatusParseError {
                input: other.to_string(),
            }),
        }
    }
}

/// Raised when [`MilestoneStatus::parse`] sees a string that does
/// not match any variant.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error(
    "invalid milestone status '{input}': expected one of planning / active / on_hold / completed"
)]
pub struct MilestoneStatusParseError {
    /// The offending input string echoed back for user-facing
    /// errors.
    pub input: String,
}

/// `Status` trait impl forwards to the inherent methods, mirroring
/// `FeatureStatus` and `IssueStatus`.
impl Status for MilestoneStatus {
    type ParseError = MilestoneStatusParseError;

    fn as_str(self) -> &'static str {
        MilestoneStatus::as_str(self)
    }

    fn is_default_hidden(self) -> bool {
        MilestoneStatus::is_default_hidden(self)
    }

    fn all() -> &'static [Self] {
        MilestoneStatus::all()
    }

    fn parse(raw: &str) -> Result<Self, Self::ParseError> {
        MilestoneStatus::parse(raw)
    }
}

/// Structured block describing a milestone, carried inside
/// [`MemoryFrontmatter::milestone`](crate::memory::MemoryFrontmatter)
/// when the memory's `kind` is `Milestone`.
///
/// Deliberately reduced relative to [`FeatureMetadata`](super::FeatureMetadata) /
/// [`IssueMetadata`](super::IssueMetadata): no `number`, no
/// `depends_on` / `blocks`, no `superseded_by`. The one field it
/// does carry is the operator-set editorial status; the live,
/// computed rollup status lives in `mmcp_store::rollup` instead of
/// frontmatter.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MilestoneMetadata {
    /// Current editorial state, set directly via `add_milestone` /
    /// `update_milestone`.
    #[serde(default)]
    pub status: MilestoneStatus,
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn status_round_trips_through_str_and_parse() {
        for variant in MilestoneStatus::all() {
            let parsed = MilestoneStatus::parse(variant.as_str()).expect("round trip");
            assert_eq!(&parsed, variant);
        }
    }

    #[test]
    fn status_parse_rejects_unknown() {
        let err = MilestoneStatus::parse("blocked").expect_err("unknown status must fail");
        assert_eq!(err.input, "blocked");
    }

    #[test]
    fn default_metadata_is_planning() {
        let meta = MilestoneMetadata::default();
        assert_eq!(meta.status, MilestoneStatus::Planning);
    }

    #[test]
    fn only_completed_is_default_hidden() {
        for variant in MilestoneStatus::all() {
            let hidden = variant.is_default_hidden();
            match variant {
                MilestoneStatus::Completed => assert!(hidden, "{variant:?} must be default-hidden"),
                MilestoneStatus::Planning | MilestoneStatus::Active | MilestoneStatus::OnHold => {
                    assert!(!hidden, "{variant:?} must stay visible by default")
                }
            }
        }
    }

    #[test]
    fn metadata_round_trips_through_toml() {
        let meta = MilestoneMetadata {
            status: MilestoneStatus::Active,
        };
        let rendered = toml::to_string(&meta).expect("render");
        let parsed: MilestoneMetadata = toml::from_str(&rendered).expect("parse");
        assert_eq!(parsed, meta);
    }

    #[test]
    fn status_trait_forwards_match_inherent_impl() {
        for variant in MilestoneStatus::all() {
            let s: &str = <MilestoneStatus as Status>::as_str(*variant);
            assert_eq!(s, variant.as_str());
        }
    }
}
