//! Cross-group milestone rollup.
//!
//! A milestone's features are not required to live in the same
//! project group as the milestone itself (D3, the operator's
//! explicit ruling overruling the more cautious mono-group default).
//! Computing "what is this milestone's status right now" therefore
//! means folding over feature memories that may be scattered across
//! every locally-mirrored group. Live-walking every group's git
//! repository on every rollup computation would not scale, so this
//! module queries the [`crate::cache`] local content index instead —
//! a single `indexed_memory` table scan (`WHERE kind = 'feature' AND
//! milestone = ?`) that already spans every mirrored group by
//! construction (see [`crate::cache::schema`]'s `group_id` column).
//!
//! Per global-coding-rules section 13 this lives in its own
//! concern-named module because it has two consumers from day one:
//! [`crate::milestones::list_milestones`] (and `read_milestone`) and
//! the `diagnose` milestone check in [`crate::diagnostics`]. Neither
//! owns the rollup rule; both call this module's [`compute`].
//!
//! ## The rollup rule (design decision, not fully specified by the
//! ## originating FR — documented here for the next reader)
//!
//! A milestone's computed [`RollupStatus`] folds over the
//! [`FeatureStatus`](mmcp_core::memory::FeatureStatus) of every
//! feature currently pointing at it:
//!
//! 1. `Duplicate` and `Superseded` features are excluded from the
//!    fold entirely. Both statuses mean "this ticket does not
//!    represent live remaining work" (a duplicate was filed by
//!    accident; a superseded feature was replaced by another one
//!    that, if it also targets this milestone, is already counted
//!    on its own). Counting them would double-count or count dead
//!    weight.
//! 2. If nothing counts (no feature points at the milestone, or
//!    every pointing feature was excluded by rule 1), the rollup is
//!    [`RollupStatus::Planning`] — there is no live work yet.
//! 3. Otherwise, if ANY counted feature is
//!    [`FeatureStatus::Blocked`], the rollup is
//!    [`RollupStatus::Blocked`]. Blocked wins over every other
//!    state because it is the one state that needs an operator's
//!    attention right now — a milestone with 9 completed features
//!    and 1 blocked one is not "almost done", it is "stuck".
//! 4. Otherwise, if EVERY counted feature is
//!    [`FeatureStatus::Completed`], the rollup is
//!    [`RollupStatus::Completed`].
//! 5. Otherwise (a mix of `Requested` / `Approved` / `Pending` /
//!    `Deferred`, with no `Blocked` present and not everything
//!    `Completed`), the rollup is [`RollupStatus::InProgress`].
//!
//! This is a strict, order-independent fold: the same feature set
//! always produces the same rollup regardless of scan order, which
//! matters because the underlying cache query has no guaranteed row
//! order across groups.

use serde::{Deserialize, Serialize};
use sqlx::sqlite::SqlitePool;
use uuid::Uuid;

use mmcp_core::memory::FeatureStatus;
use mmcp_git::NativeBackend;

use crate::cache::CacheError;
use crate::groups::GroupIndex;

/// Computed status of a milestone, folded over the lifecycle states
/// of every feature (in any locally-mirrored group) whose
/// [`FeatureMetadata::milestone`](mmcp_core::memory::FeatureMetadata::milestone)
/// points at it. See the module doc for the exact fold rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RollupStatus {
    /// No feature currently counts toward this milestone.
    Planning,
    /// At least one counted feature is `Blocked`.
    Blocked,
    /// Every counted feature is `Completed`, and at least one
    /// feature counts.
    Completed,
    /// A mix of non-terminal states, with no `Blocked` present.
    InProgress,
}

impl RollupStatus {
    /// Canonical lowercase string, matching the serde `snake_case`
    /// serialization.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            RollupStatus::Planning => "planning",
            RollupStatus::Blocked => "blocked",
            RollupStatus::Completed => "completed",
            RollupStatus::InProgress => "in_progress",
        }
    }
}

/// Full result of folding a milestone's linked features, returned
/// by [`compute`]. Carries the raw counts alongside the folded
/// [`RollupStatus`] so a caller can render "7/9 features completed"
/// without a second query.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MilestoneRollup {
    pub status: RollupStatus,
    /// Number of features that counted toward the fold (excludes
    /// `Duplicate` / `Superseded`).
    pub counted: usize,
    pub completed: usize,
    pub blocked: usize,
}

/// Compute `milestone_id`'s rollup by scanning the local content
/// cache across every locally-mirrored group. Lazily builds the
/// cache first (see [`crate::cache::ensure_built`]) so a cold cache
/// never returns a false [`RollupStatus::Planning`].
pub async fn compute(
    pool: &SqlitePool,
    backend: &NativeBackend,
    groups: &GroupIndex,
    milestone_id: Uuid,
) -> Result<MilestoneRollup, CacheError> {
    crate::cache::ensure_built(pool, backend, groups).await?;
    let statuses = fetch_feature_statuses(pool, milestone_id).await?;
    Ok(fold(&statuses))
}

/// Query-only half of [`compute`], split out so the fold logic
/// itself (the part with the actual rule, and the part worth unit
/// testing in isolation) never needs a live pool.
async fn fetch_feature_statuses(
    pool: &SqlitePool,
    milestone_id: Uuid,
) -> Result<Vec<FeatureStatus>, CacheError> {
    let rows: Vec<(Option<String>,)> = sqlx::query_as(
        "SELECT status FROM indexed_memory WHERE kind = 'feature' AND milestone = ?",
    )
    .bind(milestone_id.to_string())
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .filter_map(|(raw,)| raw.and_then(|s| FeatureStatus::parse(&s).ok()))
        .collect())
}

/// The rollup rule itself. See the module doc for the full
/// rationale; kept as a free function over a plain slice so it is
/// trivially unit-testable without a database.
fn fold(statuses: &[FeatureStatus]) -> MilestoneRollup {
    let counted: Vec<FeatureStatus> = statuses
        .iter()
        .copied()
        .filter(|s| !matches!(s, FeatureStatus::Duplicate | FeatureStatus::Superseded))
        .collect();

    let completed = counted
        .iter()
        .filter(|s| **s == FeatureStatus::Completed)
        .count();
    let blocked = counted
        .iter()
        .filter(|s| **s == FeatureStatus::Blocked)
        .count();

    let status = if counted.is_empty() {
        RollupStatus::Planning
    } else if blocked > 0 {
        RollupStatus::Blocked
    } else if completed == counted.len() {
        RollupStatus::Completed
    } else {
        RollupStatus::InProgress
    };

    MilestoneRollup {
        status,
        counted: counted.len(),
        completed,
        blocked,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_set_is_planning() {
        let rollup = fold(&[]);
        assert_eq!(rollup.status, RollupStatus::Planning);
        assert_eq!(rollup.counted, 0);
    }

    #[test]
    fn only_duplicate_and_superseded_folds_to_planning() {
        let rollup = fold(&[FeatureStatus::Duplicate, FeatureStatus::Superseded]);
        assert_eq!(rollup.status, RollupStatus::Planning);
        assert_eq!(rollup.counted, 0);
    }

    #[test]
    fn any_blocked_wins_over_completed() {
        let rollup = fold(&[
            FeatureStatus::Completed,
            FeatureStatus::Completed,
            FeatureStatus::Blocked,
        ]);
        assert_eq!(rollup.status, RollupStatus::Blocked);
        assert_eq!(rollup.counted, 3);
        assert_eq!(rollup.blocked, 1);
    }

    #[test]
    fn all_completed_is_completed() {
        let rollup = fold(&[FeatureStatus::Completed, FeatureStatus::Completed]);
        assert_eq!(rollup.status, RollupStatus::Completed);
        assert_eq!(rollup.completed, 2);
    }

    #[test]
    fn mixed_non_blocked_is_in_progress() {
        let rollup = fold(&[
            FeatureStatus::Completed,
            FeatureStatus::Requested,
            FeatureStatus::Deferred,
        ]);
        assert_eq!(rollup.status, RollupStatus::InProgress);
    }

    #[test]
    fn duplicate_and_superseded_are_excluded_from_the_denominator() {
        // A milestone with one completed feature and one superseded
        // one is Completed, not InProgress: the superseded feature
        // does not represent live remaining work.
        let rollup = fold(&[FeatureStatus::Completed, FeatureStatus::Superseded]);
        assert_eq!(rollup.status, RollupStatus::Completed);
        assert_eq!(rollup.counted, 1);
    }

    #[test]
    fn status_as_str_matches_snake_case() {
        assert_eq!(RollupStatus::InProgress.as_str(), "in_progress");
        assert_eq!(RollupStatus::Planning.as_str(), "planning");
        assert_eq!(RollupStatus::Blocked.as_str(), "blocked");
        assert_eq!(RollupStatus::Completed.as_str(), "completed");
    }
}
