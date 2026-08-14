//! Milestone rollup.
//!
//! [`compute`] counts only features whose `group_id` matches the milestone's own group.
//! Cross-group counting allows spoofing a victim milestone's status via an unprotected sibling group.
//! A caller with write access to their own group can file a feature there,
//! targeting the victim's milestone UUID with `status: "blocked"`.
//! The victim group then reports a status it never actually reached, with no access needed to its own group.
//! A cross-group authorization/allowlist model is a separate, unbuilt feature.
//!
//! Queries the [`crate::cache`] local content index instead of walking git live,
//! filtered by `kind = 'feature'`, `milestone`, `group_id` on the `indexed_memory` table.
//!
//! Consumers: see also: crate::milestones::read_milestone, crate::milestones::list_milestones,
//! and the milestone check in crate::diagnostics.
//!
//! ## Rollup rule
//!
//! [`RollupStatus`] folds over every feature's [`FeatureStatus`] pointing at this milestone:
//!
//! 1. `Duplicate` and `Superseded` features are excluded: neither represents live remaining work.
//! 2. If nothing counts, the rollup is [`RollupStatus::Planning`].
//! 3. Otherwise, if any counted feature is [`FeatureStatus::Blocked`],
//!    the rollup is [`RollupStatus::Blocked`], which wins over every other state.
//! 4. Otherwise, if every counted feature is [`FeatureStatus::Completed`],
//!    the rollup is [`RollupStatus::Completed`].
//! 5. Otherwise, the rollup is [`RollupStatus::InProgress`].
//!
//! Order-independent fold: the same feature set always produces the same rollup regardless of scan order,
//! since the cache query has no guaranteed row order across groups.

use serde::{Deserialize, Serialize};
use sqlx::sqlite::SqlitePool;
use uuid::Uuid;

use mmcp_core::memory::FeatureStatus;
use mmcp_git::NativeBackend;

use crate::cache::CacheError;
use crate::groups::GroupIndex;

/// Computed status of a milestone.
/// Folds over the lifecycle state of every feature in the milestone's own group,
/// linked via [`FeatureMetadata::milestone`](mmcp_core::memory::FeatureMetadata::milestone).
/// See the module doc for the exact fold rule and for why the scope is restricted to one group.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RollupStatus {
    /// No feature currently counts toward this milestone.
    Planning,
    /// At least one counted feature is `Blocked`.
    Blocked,
    /// Every counted feature is `Completed`, and at least one feature counts.
    Completed,
    /// A mix of non-terminal states, with no `Blocked` present.
    InProgress,
}

impl RollupStatus {
    /// Canonical lowercase string, matching the serde `snake_case` serialization.
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

/// Full result of folding a milestone's linked features, returned by [`compute`].
/// Carries the raw counts alongside the folded [`RollupStatus`],
/// so a caller can render "7/9 features completed" without a second query.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MilestoneRollup {
    pub status: RollupStatus,
    /// Number of features that counted toward the fold (excludes `Duplicate` / `Superseded`).
    pub counted: usize,
    pub completed: usize,
    pub blocked: usize,
}

/// Compute `milestone_id`'s rollup by scanning the local content cache,
/// counting only features whose `group_id` matches `owner_group_id`,
/// the group the milestone itself lives in.
/// See the module doc for why the scope is restricted this way.
/// Lazily builds the cache first (see [`crate::cache::ensure_built`]).
/// A cold cache never returns a false [`RollupStatus::Planning`].
pub async fn compute(
    pool: &SqlitePool,
    backend: &NativeBackend,
    groups: &GroupIndex,
    owner_group_id: Uuid,
    milestone_id: Uuid,
) -> Result<MilestoneRollup, CacheError> {
    crate::cache::ensure_built(pool, backend, groups).await?;
    let statuses = fetch_feature_statuses(pool, owner_group_id, milestone_id).await?;
    Ok(fold(&statuses))
}

/// Query-only half of [`compute`].
/// Split out so the fold logic, the actual rule and the part worth unit testing, never needs a live pool.
///
/// A row with no status at all (`NULL`) is skipped.
/// `kind = 'feature'` rows are only ever written without a status by data that predates the column.
/// The schema doc already treats that as a legitimate absence, not corruption.
///
/// A row that DOES carry a status string that fails [`FeatureStatus::parse`] is a different case:
/// a real feature whose lifecycle state cannot be read.
/// It surfaces as [`CacheError::UnparseableFeatureStatus`] instead of silently excluding the row.
/// Otherwise a rollup could report a milestone `Completed`,
/// while a real `Blocked` feature stays invisible because its status string no longer parses.
async fn fetch_feature_statuses(
    pool: &SqlitePool,
    owner_group_id: Uuid,
    milestone_id: Uuid,
) -> Result<Vec<FeatureStatus>, CacheError> {
    let rows: Vec<(Option<String>,)> = sqlx::query_as(
        "SELECT status FROM indexed_memory \
         WHERE kind = 'feature' AND milestone = ? AND group_id = ?",
    )
    .bind(milestone_id.to_string())
    .bind(owner_group_id.to_string())
    .fetch_all(pool)
    .await?;
    rows.into_iter()
        .filter_map(|(raw,)| raw)
        .map(|raw| {
            FeatureStatus::parse(&raw)
                .map_err(|source| CacheError::UnparseableFeatureStatus { raw, source })
        })
        .collect()
}

/// The rollup rule itself.
/// See the module doc for the full rationale.
/// Kept as a free function over a plain slice, trivially unit-testable without a database.
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
        // A milestone with one completed feature and one superseded feature is Completed, not InProgress:
        // the superseded feature does not represent live remaining work.
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

    #[tokio::test]
    async fn fetch_feature_statuses_surfaces_a_row_whose_status_fails_to_parse() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let pool = crate::cache::open_pool(&tmp.path().join("index.sqlite3"))
            .await
            .expect("open pool");
        let group_id = Uuid::now_v7();
        let milestone_id = Uuid::now_v7();

        sqlx::query(
            "INSERT INTO indexed_memory \
               (group_id, id, slug, kind, name, description, tags, body, path, \
                commit_id, updated_at, status, milestone) \
             VALUES (?, ?, ?, 'feature', ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(group_id.to_string())
        .bind(Uuid::now_v7().to_string())
        .bind("broken-feature")
        .bind("broken feature")
        .bind("a feature with a status this cache cannot parse")
        .bind("[]")
        .bind("")
        .bind("memories/broken-feature/x.md")
        .bind("HEAD")
        .bind("2026-08-06T00:00:00Z")
        .bind("not-a-real-status")
        .bind(milestone_id.to_string())
        .execute(&pool)
        .await
        .expect("seed row with unparseable status");

        let err = fetch_feature_statuses(&pool, group_id, milestone_id)
            .await
            .expect_err("an unparseable status must surface as an error, not be dropped");
        match err {
            CacheError::UnparseableFeatureStatus { raw, .. } => {
                assert_eq!(raw, "not-a-real-status");
            }
            other => panic!("expected UnparseableFeatureStatus, got {other:?}"),
        }
    }

    async fn seed_feature_row(
        pool: &SqlitePool,
        group_id: Uuid,
        milestone_id: Uuid,
        slug: &str,
        status: FeatureStatus,
    ) {
        sqlx::query(
            "INSERT INTO indexed_memory \
               (group_id, id, slug, kind, name, description, tags, body, path, \
                commit_id, updated_at, status, milestone) \
             VALUES (?, ?, ?, 'feature', ?, ?, '[]', '', ?, 'HEAD', \
                      '2026-08-06T00:00:00Z', ?, ?)",
        )
        .bind(group_id.to_string())
        .bind(Uuid::now_v7().to_string())
        .bind(slug)
        .bind(slug)
        .bind(format!("a feature named {slug}"))
        .bind(format!("memories/{slug}/x.md"))
        .bind(status.as_str())
        .bind(milestone_id.to_string())
        .execute(pool)
        .await
        .expect("seed feature row");
    }

    #[tokio::test]
    async fn compute_ignores_a_feature_filed_in_a_different_group() {
        // Security regression.
        // A feature filed in group B and pointed at group A's milestone must never influence group A's rollup.
        // Without the group_id predicate this reproduces the cross-group rollup-injection finding,
        // where a Blocked feature in an unrelated, unprotected group flips the victim milestone off Completed.
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let pool = crate::cache::open_pool(&tmp.path().join("index.sqlite3"))
            .await
            .expect("open pool");

        let group_a = Uuid::now_v7();
        let group_b = Uuid::now_v7();
        let milestone_id = Uuid::now_v7();

        seed_feature_row(
            &pool,
            group_a,
            milestone_id,
            "owning-group-feature",
            FeatureStatus::Completed,
        )
        .await;
        seed_feature_row(
            &pool,
            group_b,
            milestone_id,
            "foreign-group-feature",
            FeatureStatus::Blocked,
        )
        .await;

        let statuses = fetch_feature_statuses(&pool, group_a, milestone_id)
            .await
            .expect("fetch_feature_statuses");
        assert_eq!(
            statuses,
            vec![FeatureStatus::Completed],
            "the foreign group's Blocked feature must not be counted"
        );

        let rollup = fold(&statuses);
        assert_eq!(
            rollup.status,
            RollupStatus::Completed,
            "a foreign-group feature must not flip the rollup away from Completed"
        );
    }
}
