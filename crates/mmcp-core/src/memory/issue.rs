//! Typed metadata for issue-tracker memories.
//!
//! Sister surface to [`feature`](super::feature) — issues record
//! bugs, questions, chores, and operational concerns alongside
//! feature requests in the same group, with a deliberately
//! different status set: issues "close" rather than "resolve",
//! and they get a terminal "wontfix" verdict that features have
//! no equivalent for.
//!
//! Cross-references (`depends_on` / `blocks`) are kind-agnostic
//! UUID lists, so an issue can depend on a feature or vice versa.
//! The hybrid model from the design discussion permits a memory
//! to carry both a `[feature]` and an `[issue]` block; the listing
//! surfaces filter by block presence rather than `kind` value.
//!
//! `IssueStatus` and `IssueMetadata` live in the same file because
//! the struct exists solely to group the status, the typed
//! cross-references, and the supersede back-link; splitting them
//! across files would add no clarity.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::memory::{MemoryRef, Status};

/// Lifecycle state of an issue.
///
/// Differs from [`FeatureStatus`](super::FeatureStatus) on the
/// terminal verbs: issues *close* (the work landed, the question
/// got an answer) and may be *wontfix*ed (the team explicitly
/// declines to act). Open / Blocked / Deferred / Duplicate /
/// Superseded carry the same semantics as on the feature side so
/// generic listings and diagnostics behave identically.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum IssueStatus {
    /// Active issue; not yet closed, not yet declined.
    #[default]
    Open,
    /// Terminal: was acted on (bug fixed, question answered,
    /// chore done). The memory stays in the group for history.
    Closed,
    /// Terminal: explicit refusal to act. Distinct from `Closed`
    /// so a listing or audit can tell the two outcomes apart.
    Wontfix,
    /// Waiting on an external prerequisite.
    Blocked,
    /// Intentionally postponed but still valid.
    Deferred,
    /// Duplicate of another issue. Body usually carries a pointer
    /// to the surviving slug.
    Duplicate,
    /// Replaced by a newer issue (or feature) via the typed
    /// supersede flow. [`IssueMetadata::superseded_by`] points at
    /// the replacement.
    Superseded,
}

impl IssueStatus {
    /// Canonical lowercase string, matching the serde
    /// `snake_case` serialization.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            IssueStatus::Open => "open",
            IssueStatus::Closed => "closed",
            IssueStatus::Wontfix => "wontfix",
            IssueStatus::Blocked => "blocked",
            IssueStatus::Deferred => "deferred",
            IssueStatus::Duplicate => "duplicate",
            IssueStatus::Superseded => "superseded",
        }
    }

    /// Every variant in declaration order.
    #[must_use]
    pub const fn all() -> &'static [IssueStatus] {
        &[
            IssueStatus::Open,
            IssueStatus::Closed,
            IssueStatus::Wontfix,
            IssueStatus::Blocked,
            IssueStatus::Deferred,
            IssueStatus::Duplicate,
            IssueStatus::Superseded,
        ]
    }

    /// Statuses hidden from the default `list_issues(all = false)`
    /// listing. Closed, Wontfix, Duplicate, and Superseded all
    /// represent terminal verdicts; an operator pulling them back
    /// into the listing does so explicitly via the `all` flag or
    /// a direct `status` filter.
    #[must_use]
    pub const fn is_default_hidden(self) -> bool {
        matches!(
            self,
            IssueStatus::Closed
                | IssueStatus::Wontfix
                | IssueStatus::Duplicate
                | IssueStatus::Superseded,
        )
    }

    /// Parse the lowercase wire form back into a variant.
    pub fn parse(raw: &str) -> Result<Self, IssueStatusParseError> {
        match raw {
            "open" => Ok(IssueStatus::Open),
            "closed" => Ok(IssueStatus::Closed),
            "wontfix" => Ok(IssueStatus::Wontfix),
            "blocked" => Ok(IssueStatus::Blocked),
            "deferred" => Ok(IssueStatus::Deferred),
            "duplicate" => Ok(IssueStatus::Duplicate),
            "superseded" => Ok(IssueStatus::Superseded),
            other => Err(IssueStatusParseError {
                input: other.to_string(),
            }),
        }
    }
}

/// Raised when [`IssueStatus::parse`] sees a string that does not
/// match any variant.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error(
    "invalid issue status '{input}': expected one of open / closed / wontfix / blocked / deferred / duplicate / superseded"
)]
pub struct IssueStatusParseError {
    /// The offending input string echoed back for user-facing
    /// errors.
    pub input: String,
}

/// `Status` trait impl forwards to the inherent methods so generic
/// helpers over `T: Status` see the issue contract identically to
/// the feature one.
impl Status for IssueStatus {
    type ParseError = IssueStatusParseError;

    fn as_str(self) -> &'static str {
        IssueStatus::as_str(self)
    }

    fn is_default_hidden(self) -> bool {
        IssueStatus::is_default_hidden(self)
    }

    fn all() -> &'static [Self] {
        IssueStatus::all()
    }

    fn parse(raw: &str) -> Result<Self, Self::ParseError> {
        IssueStatus::parse(raw)
    }
}

/// Structured block describing an issue, carried inside
/// [`MemoryFrontmatter::issue`](crate::memory::MemoryFrontmatter)
/// when the memory's `kind` is `Issue` or when the memory is a
/// hybrid ticket carrying both a `[feature]` and an `[issue]`
/// block.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct IssueMetadata {
    /// Current lifecycle state.
    #[serde(default)]
    pub status: IssueStatus,

    /// Sequential number per group, server-assigned. Shared
    /// counter with features so the project enjoys one ticket
    /// number space across both kinds (GitHub-style). Auto-
    /// assigned at create time as `max(existing_numbers) + 1`
    /// across every tracker memory in the group.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub number: Option<u32>,

    /// UUIDs of memories this issue depends on. Cross-kind: the
    /// target may be a feature, another issue, or any future
    /// tracker kind.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub depends_on: Vec<Uuid>,

    /// UUIDs of memories whose own resolution is gated on this
    /// issue. Inverse of `depends_on`, kept explicit so neither
    /// graph direction needs a scan.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blocks: Vec<Uuid>,

    /// Typed back-link to the memory that replaced this issue,
    /// set by the supersede flow on `add_issue`. Paired with
    /// [`IssueStatus::Superseded`] by
    /// [`IssueMetadata::validate_supersede_invariant`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub superseded_by: Option<MemoryRef>,
}

impl IssueMetadata {
    /// Enforce the paired invariant between `status` and
    /// `superseded_by`: either both signal supersession or
    /// neither does. Mirrors the feature-side check.
    pub fn validate_supersede_invariant(&self) -> Result<(), IssueSupersedeInvariantError> {
        match (self.status, self.superseded_by.is_some()) {
            (IssueStatus::Superseded, true) => Ok(()),
            (IssueStatus::Superseded, false) => {
                Err(IssueSupersedeInvariantError::MissingSupersededBy)
            }
            (other, true) => {
                Err(IssueSupersedeInvariantError::UnexpectedSupersededBy { status: other })
            }
            (_, false) => Ok(()),
        }
    }
}

/// Raised by [`IssueMetadata::validate_supersede_invariant`] when
/// `status` and `superseded_by` disagree on whether the issue is
/// superseded.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum IssueSupersedeInvariantError {
    /// `status == Superseded` but no back-link is set.
    #[error("status is superseded but superseded_by is empty")]
    MissingSupersededBy,
    /// `superseded_by` is set but `status` is not `Superseded`.
    #[error("superseded_by is set but status is {status:?}, expected superseded")]
    UnexpectedSupersededBy { status: IssueStatus },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn forty_char_hex() -> &'static str {
        "0123456789abcdef0123456789abcdef01234567"
    }

    #[test]
    fn status_round_trips_through_str_and_parse() {
        for variant in IssueStatus::all() {
            let parsed = IssueStatus::parse(variant.as_str()).expect("round trip");
            assert_eq!(&parsed, variant);
        }
    }

    #[test]
    fn status_parse_rejects_unknown() {
        let err = IssueStatus::parse("invented").expect_err("unknown must fail");
        assert_eq!(err.input, "invented");
    }

    #[test]
    fn default_metadata_is_open_with_empty_cross_refs() {
        let meta = IssueMetadata::default();
        assert_eq!(meta.status, IssueStatus::Open);
        assert!(meta.depends_on.is_empty());
        assert!(meta.blocks.is_empty());
        assert_eq!(meta.number, None);
        assert!(meta.superseded_by.is_none());
    }

    #[test]
    fn is_default_hidden_covers_terminal_states() {
        for variant in IssueStatus::all() {
            let hidden = variant.is_default_hidden();
            match variant {
                IssueStatus::Closed
                | IssueStatus::Wontfix
                | IssueStatus::Duplicate
                | IssueStatus::Superseded => {
                    assert!(hidden, "{variant:?} must be default-hidden")
                }
                IssueStatus::Open | IssueStatus::Blocked | IssueStatus::Deferred => {
                    assert!(!hidden, "{variant:?} must stay visible by default")
                }
            }
        }
    }

    #[test]
    fn metadata_round_trips_through_toml() {
        let meta = IssueMetadata {
            status: IssueStatus::Wontfix,
            number: Some(7),
            depends_on: vec![Uuid::now_v7()],
            blocks: vec![Uuid::now_v7()],
            superseded_by: None,
        };
        let rendered = toml::to_string(&meta).expect("render");
        let parsed: IssueMetadata = toml::from_str(&rendered).expect("parse");
        assert_eq!(parsed, meta);
    }

    #[test]
    fn supersede_invariant_accepts_both_set() {
        let meta = IssueMetadata {
            status: IssueStatus::Superseded,
            number: None,
            depends_on: Vec::new(),
            blocks: Vec::new(),
            superseded_by: Some(MemoryRef::new(Uuid::now_v7(), forty_char_hex())),
        };
        assert!(meta.validate_supersede_invariant().is_ok());
    }

    #[test]
    fn supersede_invariant_rejects_status_without_link() {
        let meta = IssueMetadata {
            status: IssueStatus::Superseded,
            ..Default::default()
        };
        let err = meta
            .validate_supersede_invariant()
            .expect_err("status without link must fail");
        assert_eq!(err, IssueSupersedeInvariantError::MissingSupersededBy);
    }

    #[test]
    fn supersede_invariant_rejects_link_without_status() {
        let meta = IssueMetadata {
            superseded_by: Some(MemoryRef::new(Uuid::now_v7(), forty_char_hex())),
            ..Default::default()
        };
        let err = meta
            .validate_supersede_invariant()
            .expect_err("link without status must fail");
        assert_eq!(
            err,
            IssueSupersedeInvariantError::UnexpectedSupersededBy {
                status: IssueStatus::Open,
            }
        );
    }

    #[test]
    fn status_trait_forwards_match_inherent_impl() {
        for variant in IssueStatus::all() {
            let s: &str = <IssueStatus as Status>::as_str(*variant);
            assert_eq!(s, variant.as_str());
        }
    }
}
