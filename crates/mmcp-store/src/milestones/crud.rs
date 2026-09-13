//! Typed CRUD over milestone memories.
//!
//! A milestone's own frontmatter carries only an editorial [`MilestoneStatus`].
//! Its LIVE, computed status, the fold over the lifecycle states of every feature,
//! in the milestone's own group pointing at it (see [`super::rollup`]),
//! is never persisted;
//! every read path in this module computes it fresh via [`super::rollup`],
//! and attaches it to the returned record.

use mmcp_core::memory::{
    FrontmatterFormat, MemoryFile, MemoryFrontmatter, MemoryKind, MilestoneMetadata,
    MilestoneStatus,
};
use mmcp_git::{GitBackend, NativeBackend, Rev};
use sqlx::sqlite::SqlitePool;
use uuid::Uuid;

use super::rollup::{self, MilestoneRollup, RollupStatus};
use crate::diagnostics::Finding;
use crate::groups::{GroupEntry, GroupIndex};
use crate::home::ResolvedAuthor;
use crate::memory::{
    AddressingMode, ImportError, WriteFileOptions, WriteMemoryOptions, resolve_memory,
    slugify_filename, validate_memory_slug, write_file_at_path, write_memory_by_id,
};

/// Errors specific to milestone operations.
#[derive(Debug, thiserror::Error)]
pub enum MilestoneError {
    /// Propagated from the memory CRUD primitives.
    #[error(transparent)]
    Memory(#[from] ImportError),

    /// Propagated from a rollup computation against the local
    /// content cache.
    #[error(transparent)]
    Cache(#[from] crate::cache::CacheError),

    /// Raised when `read_milestone`/`update_milestone` target a memory that exists but is not a `Milestone` kind.
    /// Keeps the milestone tools from silently operating on unrelated memories.
    #[error("memory '{slug}' exists in this group but does not carry a milestone metadata block")]
    NotAMilestone { slug: String, kind: String },

    /// `add_milestone` was called without a title and without a
    /// slug.
    #[error("milestone title is required when no slug is provided")]
    TitleRequired,

    /// `update_milestone` was called with every mutator field in
    /// [`UpdateSpec`] absent. Caught before `update_milestone` touches
    /// the lock chain or the backend, so no read, write, or commit
    /// is ever attempted for the no-op call.
    #[error(
        "update_milestone requires at least one mutator field; all fields were omitted or empty"
    )]
    NoChangesSupplied,
}

/// Input for [`add_milestone`].
#[derive(Debug, Clone, Default)]
pub struct AddSpec {
    pub slug: Option<String>,
    pub title: String,
    pub description: String,
    pub body: String,
    pub status: MilestoneStatus,
    pub message: Option<String>,
}

/// Input for [`update_milestone`].
/// Every field is optional; `Some(v)` replaces, `None` leaves untouched.
#[derive(Debug, Clone, Default)]
pub struct UpdateSpec {
    pub title: Option<String>,
    pub description: Option<String>,
    pub body: Option<String>,
    pub status: Option<MilestoneStatus>,
    pub message: Option<String>,
}

impl UpdateSpec {
    /// True when at least one mutator field is set.
    /// `message` only overrides the commit message text and never
    /// counts as a change on its own.
    fn has_any_change(&self) -> bool {
        self.title.is_some()
            || self.description.is_some()
            || self.body.is_some()
            || self.status.is_some()
    }
}

/// Typed return shape for every read / write path on the milestone
/// surface, always carrying the freshly-computed [`MilestoneRollup`]
/// alongside the on-disk editorial [`MilestoneStatus`] so callers
/// can compare the two without a second query.
#[derive(Debug, Clone)]
pub struct MilestoneRecord {
    pub slug: String,
    pub title: String,
    pub description: String,
    pub body: String,
    pub status: MilestoneStatus,
    pub rollup: MilestoneRollup,
    pub commit_id: String,
}

/// Create a new milestone in the group.
/// Errors with `MilestoneError::Memory(ImportError::MemoryAlreadyExists)`,
/// when the slug already points at something on disk.
///
/// The returned record's rollup is always the trivial zero-features [`RollupStatus::Planning`]:
/// a freshly-minted UUID cannot yet have any feature pointing at it,
/// so this path skips the cache query entirely,
/// rather than pay for a lookup that can only ever come back empty.
pub async fn add_milestone(
    backend: &NativeBackend,
    entry: &GroupEntry,
    spec: AddSpec,
    author: &ResolvedAuthor,
) -> Result<MilestoneRecord, MilestoneError> {
    let group = *entry.manifest.group_id.as_uuid();
    let _guards = crate::lock::acquire_chain(&crate::lock::create_chain(group)).await;

    if spec.title.trim().is_empty() && spec.slug.is_none() {
        return Err(MilestoneError::TitleRequired);
    }
    let slug = match spec.slug.clone() {
        Some(raw) => raw,
        None => slugify_filename(&spec.title),
    };
    validate_memory_slug(&slug).map_err(MilestoneError::Memory)?;

    let id = Uuid::now_v7();
    let metadata = MilestoneMetadata {
        status: spec.status,
    };
    let mut file = build_memory_file(
        spec.title.clone(),
        spec.description.clone(),
        spec.body.clone(),
        metadata,
    );
    file.frontmatter = file.frontmatter.clone().with_id(id);
    let rendered = file
        .to_string()
        .map_err(|e| MilestoneError::Memory(ImportError::Render(e.to_string())))?;

    let message = spec
        .message
        .clone()
        .unwrap_or_else(|| format!("create milestone {slug}"));
    let (commit_id, _validation) = write_memory_by_id(
        backend,
        &entry.handle,
        &slug,
        id,
        &rendered,
        author,
        WriteMemoryOptions {
            addressing_mode: AddressingMode::BySlugOnly,
            message: Some(&message),
            ..Default::default()
        },
    )
    .await?;

    Ok(MilestoneRecord {
        slug,
        title: spec.title,
        description: spec.description,
        body: spec.body,
        status: spec.status,
        rollup: MilestoneRollup {
            status: RollupStatus::Planning,
            counted: 0,
            completed: 0,
            blocked: 0,
        },
        commit_id,
    })
}

/// Read a milestone by slug, with its live rollup freshly computed
/// across every locally-mirrored group.
pub async fn read_milestone(
    backend: &NativeBackend,
    entry: &GroupEntry,
    pool: &SqlitePool,
    groups: &GroupIndex,
    slug: &str,
    rev: Option<&str>,
) -> Result<MilestoneRecord, MilestoneError> {
    validate_memory_slug(slug).map_err(MilestoneError::Memory)?;
    let resolved = resolve_memory(backend, &entry.handle, Some(slug), None)
        .await
        .map_err(MilestoneError::Memory)?;
    let git_rev = match rev {
        Some(v) => {
            if mmcp_core::memory::looks_like_commit_sha(v) {
                Rev::Commit(v.to_string())
            } else {
                Rev::Branch(v.to_string())
            }
        }
        None => Rev::head(),
    };
    let bytes = backend
        .read_file(&entry.handle, &resolved.path, &git_rev)
        .await
        .map_err(|err| match err {
            mmcp_git::GitError::PathNotFound(_) => {
                MilestoneError::Memory(ImportError::MemoryNotFound {
                    slug: Some(slug.to_string()),
                    id: Some(resolved.id),
                })
            }
            other => MilestoneError::Memory(ImportError::Git(other)),
        })?;
    let text = String::from_utf8_lossy(&bytes).into_owned();
    let file =
        MemoryFile::parse(&text).map_err(|e| MilestoneError::Memory(ImportError::Parse(e)))?;
    // Validate the `[milestone]` block is present BEFORE paying for the rollup's DB query: this
    // is pure and DB-free, so a slug that is not a milestone fails fast with `NotAMilestone`,
    // never with a cache error surfaced by a rollup query it never needed. The extracted
    // metadata is threaded into `record_from_file` below rather than re-validated there.
    let metadata = require_milestone_metadata(slug, &file)?;
    // Compute the live rollup only once validation passed: `record_from_file` still takes it as
    // a required constructor parameter (never a mutable stub a caller might forget to
    // overwrite), so no path through this module can hand back a fabricated `Planning`/0/0/0
    // placeholder.
    let owner_group_id = *entry.manifest.group_id.as_uuid();
    let rollup = rollup::compute(pool, backend, groups, owner_group_id, resolved.id).await?;
    Ok(record_from_file(
        slug,
        file,
        String::new(),
        rollup,
        metadata,
    ))
}

/// Apply partial mutations and commit a new revision.
pub async fn update_milestone(
    backend: &NativeBackend,
    entry: &GroupEntry,
    pool: &SqlitePool,
    groups: &GroupIndex,
    slug: &str,
    spec: UpdateSpec,
    author: &ResolvedAuthor,
) -> Result<MilestoneRecord, MilestoneError> {
    if !spec.has_any_change() {
        return Err(MilestoneError::NoChangesSupplied);
    }
    let group = *entry.manifest.group_id.as_uuid();
    let _ancestors = crate::lock::acquire_chain(&[
        (
            crate::lock::LockScope::Process,
            crate::lock::LockMode::Shared,
        ),
        (
            crate::lock::LockScope::Group(group),
            crate::lock::LockMode::Shared,
        ),
    ])
    .await;
    let resolved = resolve_memory(backend, &entry.handle, Some(slug), None)
        .await
        .map_err(MilestoneError::Memory)?;
    let _leaf = crate::lock::acquire(
        crate::lock::LockScope::Memory(resolved.id),
        crate::lock::LockMode::Exclusive,
    )
    .await;

    let current = read_milestone(backend, entry, pool, groups, slug, None).await?;
    let current_frontmatter =
        crate::memory::read_frontmatter_at(backend, &entry.handle, &Rev::head(), &resolved.path)
            .await
            .map_err(MilestoneError::Memory)?;

    let title = spec.title.unwrap_or(current.title);
    let description = spec.description.unwrap_or(current.description);
    let body = spec.body.unwrap_or(current.body);
    let status = spec.status.unwrap_or(current.status);

    let metadata = MilestoneMetadata { status };
    let mut file = build_memory_file(title.clone(), description.clone(), body.clone(), metadata);
    file.frontmatter = crate::tracker::carry_forward_frontmatter(
        file.frontmatter.clone().with_id(resolved.id),
        &current_frontmatter,
        // No refs-editing surface here; keep the on-disk value.
        None,
    );
    let rendered = file
        .to_string()
        .map_err(|e| MilestoneError::Memory(ImportError::Render(e.to_string())))?;

    let message = spec
        .message
        .clone()
        .unwrap_or_else(|| format!("update milestone {slug}"));
    let (commit_id, _validation) = write_file_at_path(
        backend,
        &entry.handle,
        &resolved.path,
        &rendered,
        author,
        WriteFileOptions {
            addressing_mode: resolved.addressing_mode,
            message: Some(&message),
            ..Default::default()
        },
    )
    .await?;

    Ok(MilestoneRecord {
        slug: slug.to_string(),
        title,
        description,
        body,
        status,
        rollup: current.rollup,
        commit_id,
    })
}

/// Enumerate milestones in the group, each with a freshly-computed rollup.
/// Mirrors the tracker convention: default hides `RollupStatus::Completed`,
/// (per rule 4 of the rollup fold) unless `show_all` is set.
/// A memory that IS a milestone but whose frontmatter fails to parse is not silently dropped:
/// it is excluded from the returned records,
/// but reported back as a [`Finding`] (`frontmatter_parse_failed`).
pub async fn list_milestones(
    backend: &NativeBackend,
    entry: &GroupEntry,
    pool: &SqlitePool,
    groups: &GroupIndex,
    show_all: bool,
) -> Result<(Vec<MilestoneRecord>, Vec<Finding>), MilestoneError> {
    let slug_dirs = crate::memory::list_memory_slug_dirs(backend, &entry.handle, &Rev::head())
        .await
        .map_err(|e| MilestoneError::Memory(ImportError::Git(e)))?;

    let mut out = Vec::new();
    let mut findings = Vec::new();
    for slug_dir in slug_dirs {
        match read_milestone(backend, entry, pool, groups, &slug_dir.slug, None).await {
            Ok(record) => {
                if show_all || record.rollup.status != RollupStatus::Completed {
                    out.push(record);
                }
            }
            // `NotAMilestone` is an *expected* non-match:
            // the slug is some other memory kind, not a corruption signal.
            Err(MilestoneError::NotAMilestone { .. }) => {}
            Err(MilestoneError::Memory(ImportError::Parse(err))) => {
                findings.push(crate::tracker::parse_failed_finding(
                    &entry.manifest.group_id.to_string(),
                    &slug_dir.slug,
                    &err,
                ));
            }
            Err(other) => return Err(other),
        }
    }
    out.sort_by(|a, b| a.slug.cmp(&b.slug));
    Ok((out, findings))
}

fn build_memory_file(
    title: String,
    description: String,
    body: String,
    metadata: MilestoneMetadata,
) -> MemoryFile {
    MemoryFile {
        frontmatter: MemoryFrontmatter::new(title, description, MemoryKind::Milestone)
            .with_milestone(metadata),
        body,
        format: FrontmatterFormat::TomlPlus,
    }
}

/// Gates on `[milestone]` block PRESENCE, not `frontmatter.kind`, via [`crate::tracker::require_block`],
/// mirroring `features::record_from_file` / `issues::record_from_file`'s gating exactly.
/// Pure and DB-free, so a caller can run this BEFORE paying for a rollup query
/// (see `read_milestone`), turning a non-milestone slug into a fast, deterministic
/// `NotAMilestone` regardless of cache/DB health.
fn require_milestone_metadata(
    slug: &str,
    file: &MemoryFile,
) -> Result<MilestoneMetadata, MilestoneError> {
    let kind = file.frontmatter.kind.as_str().to_string();
    crate::tracker::require_block(
        slug,
        &kind,
        file.frontmatter.milestone.clone(),
        |slug, kind| MilestoneError::NotAMilestone { slug, kind },
    )
}

/// `rollup` is a REQUIRED constructor parameter, never a mutable stub a caller overwrites after the
/// fact: the only legitimate zero-features rollup is `add_milestone`'s freshly-minted-UUID case,
/// which never routes through this function and builds its `MilestoneRecord` directly. Every other
/// caller must pass the value [`super::rollup::compute`] actually returned, so a fabricated
/// `Planning`/0/0/0 placeholder can never reach a caller unrecomputed.
///
/// `metadata` is likewise a required parameter rather than re-derived here: the caller already
/// ran [`require_milestone_metadata`] once, before paying for the rollup query, and this function
/// trusts that result instead of re-validating (and re-cloning) the same `[milestone]` block.
fn record_from_file(
    slug: &str,
    file: MemoryFile,
    commit_id: String,
    rollup: MilestoneRollup,
    metadata: MilestoneMetadata,
) -> MilestoneRecord {
    MilestoneRecord {
        slug: slug.to_string(),
        title: file.frontmatter.name,
        description: file.frontmatter.description,
        body: file.body,
        status: metadata.status,
        rollup,
        commit_id,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use crate::cache::open_pool;
    use crate::memory::import_memory;
    use crate::testing::ScratchHome;

    async fn scratch_pool() -> (tempfile::TempDir, SqlitePool) {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let pool = open_pool(&tmp.path().join("index.sqlite3"))
            .await
            .expect("open pool");
        (tmp, pool)
    }

    #[tokio::test]
    async fn add_then_read_round_trips_with_planning_rollup() {
        let scratch = ScratchHome::new().await.expect("scratch home");
        let seeded = scratch.seed_group("milestone-group").await.expect("seed");
        let entry = scratch.groups().get(&seeded.group_id).await.expect("entry");
        let (_tmp, pool) = scratch_pool().await;

        let spec = AddSpec {
            slug: Some("m1".into()),
            title: "Milestone One".into(),
            description: "round trip test".into(),
            body: "## Scope\n\nfoo\n".into(),
            status: MilestoneStatus::Planning,
            ..AddSpec::default()
        };
        let created = add_milestone(scratch.backend(), &entry, spec, scratch.author())
            .await
            .expect("add");
        assert_eq!(created.slug, "m1");
        assert_eq!(created.rollup.status, RollupStatus::Planning);
        assert_eq!(created.rollup.counted, 0);

        let loaded = read_milestone(
            scratch.backend(),
            &entry,
            &pool,
            scratch.groups(),
            "m1",
            None,
        )
        .await
        .expect("read");
        assert_eq!(loaded.title, "Milestone One");
        assert_eq!(loaded.rollup.status, RollupStatus::Planning);
    }

    #[tokio::test]
    async fn auto_mints_slug_from_title_when_absent() {
        let scratch = ScratchHome::new().await.expect("scratch home");
        let seeded = scratch.seed_group("milestone-group").await.expect("seed");
        let entry = scratch.groups().get(&seeded.group_id).await.expect("entry");

        let spec = AddSpec {
            slug: None,
            title: "Big Launch".into(),
            description: "auto slug".into(),
            body: "body".into(),
            ..AddSpec::default()
        };
        let record = add_milestone(scratch.backend(), &entry, spec, scratch.author())
            .await
            .expect("add");
        assert_eq!(record.slug, "big-launch");
    }

    #[tokio::test]
    async fn update_replaces_status_and_preserves_other_fields() {
        let scratch = ScratchHome::new().await.expect("scratch home");
        let seeded = scratch.seed_group("milestone-group").await.expect("seed");
        let entry = scratch.groups().get(&seeded.group_id).await.expect("entry");
        let (_tmp, pool) = scratch_pool().await;

        add_milestone(
            scratch.backend(),
            &entry,
            AddSpec {
                slug: Some("u-milestone".into()),
                title: "Before".into(),
                description: "unchanged".into(),
                body: "body".into(),
                ..AddSpec::default()
            },
            scratch.author(),
        )
        .await
        .expect("seed");

        let updated = update_milestone(
            scratch.backend(),
            &entry,
            &pool,
            scratch.groups(),
            "u-milestone",
            UpdateSpec {
                status: Some(MilestoneStatus::Active),
                ..UpdateSpec::default()
            },
            scratch.author(),
        )
        .await
        .expect("update");
        assert_eq!(updated.status, MilestoneStatus::Active);
        assert_eq!(updated.title, "Before");
        assert_eq!(updated.description, "unchanged");
        assert_eq!(updated.body, "body");
    }

    /// An update naming only one field leaves every other field untouched.
    /// Compares the whole frontmatter, so a field this test does not name by hand is still covered.
    /// This module has no refs-editing surface, so `refs` must survive unchanged.
    #[tokio::test]
    async fn update_preserves_frontmatter_fields_it_does_not_own() {
        let scratch = ScratchHome::new().await.expect("scratch home");
        let seeded = scratch.seed_group("milestone-group").await.expect("seed");
        let entry = scratch.groups().get(&seeded.group_id).await.expect("entry");
        let (_tmp, pool) = scratch_pool().await;

        let seeded_file = MemoryFile {
            frontmatter: crate::testing::seed_unowned_fields(
                MemoryFrontmatter::new("Before", "unchanged", MemoryKind::Milestone)
                    .with_milestone(MilestoneMetadata {
                        status: MilestoneStatus::Planning,
                    }),
            ),
            body: "body".to_string(),
            format: FrontmatterFormat::TomlPlus,
        };
        import_memory(
            scratch.backend(),
            &entry.handle,
            "tagged-milestone",
            &seeded_file.to_string().expect("render seeded milestone"),
            None,
            scratch.author(),
            false,
        )
        .await
        .expect("seed tagged milestone");
        let before = crate::testing::read_current_frontmatter(
            scratch.backend(),
            &entry.handle,
            "tagged-milestone",
        )
        .await
        .expect("read seeded frontmatter");

        update_milestone(
            scratch.backend(),
            &entry,
            &pool,
            scratch.groups(),
            "tagged-milestone",
            UpdateSpec {
                status: Some(MilestoneStatus::Active),
                ..UpdateSpec::default()
            },
            scratch.author(),
        )
        .await
        .expect("status-only update");
        let after_status = crate::testing::read_current_frontmatter(
            scratch.backend(),
            &entry.handle,
            "tagged-milestone",
        )
        .await
        .expect("read frontmatter after status-only update");
        crate::testing::assert_update_changed_only(&before, &after_status, |fm| {
            fm.milestone
                .as_mut()
                .expect("milestone block present")
                .status = MilestoneStatus::Active;
        });

        update_milestone(
            scratch.backend(),
            &entry,
            &pool,
            scratch.groups(),
            "tagged-milestone",
            UpdateSpec {
                description: Some("a different unrelated description".into()),
                ..UpdateSpec::default()
            },
            scratch.author(),
        )
        .await
        .expect("description-only update");
        let after_description = crate::testing::read_current_frontmatter(
            scratch.backend(),
            &entry.handle,
            "tagged-milestone",
        )
        .await
        .expect("read frontmatter after description-only update");
        crate::testing::assert_update_changed_only(&after_status, &after_description, |fm| {
            fm.description = "a different unrelated description".to_string();
        });
    }

    #[tokio::test]
    async fn read_refuses_when_slug_is_not_a_milestone() {
        let scratch = ScratchHome::new().await.expect("scratch home");
        let seeded = scratch.seed_group("milestone-group").await.expect("seed");
        let entry = scratch.groups().get(&seeded.group_id).await.expect("entry");
        let (_tmp, pool) = scratch_pool().await;

        crate::features::add_feature(
            scratch.backend(),
            &entry,
            crate::features::AddSpec {
                slug: Some("a-feat".into()),
                title: "feat".into(),
                description: "guard test".into(),
                body: "x".into(),
                ..crate::features::AddSpec::default()
            },
            scratch.author(),
        )
        .await
        .expect("seed feat");

        let err = read_milestone(
            scratch.backend(),
            &entry,
            &pool,
            scratch.groups(),
            "a-feat",
            None,
        )
        .await
        .expect_err("read must reject");
        assert!(matches!(err, MilestoneError::NotAMilestone { .. }));
    }

    /// Falsification test for the validation-before-rollup reorder: closes the cache pool
    /// BEFORE calling `read_milestone`, so any rollup query attempted against it fails.
    /// A non-milestone slug must still resolve to `NotAMilestone`, never a cache-originated
    /// error, proving the validation step runs before the rollup query rather than after it.
    #[tokio::test]
    async fn read_rejects_non_milestone_before_touching_a_dead_cache_pool() {
        let scratch = ScratchHome::new().await.expect("scratch home");
        let seeded = scratch.seed_group("milestone-group").await.expect("seed");
        let entry = scratch.groups().get(&seeded.group_id).await.expect("entry");
        let (_tmp, pool) = scratch_pool().await;

        crate::features::add_feature(
            scratch.backend(),
            &entry,
            crate::features::AddSpec {
                slug: Some("dead-pool-feat".into()),
                title: "feat".into(),
                description: "dead pool guard test".into(),
                body: "x".into(),
                ..crate::features::AddSpec::default()
            },
            scratch.author(),
        )
        .await
        .expect("seed feat");

        // Close the pool so any query attempted against it fails deterministically: a rollup
        // query reaching this pool would surface as `MilestoneError::Cache`, never
        // `NotAMilestone`, so a green result here proves validation ran first.
        pool.close().await;

        let err = read_milestone(
            scratch.backend(),
            &entry,
            &pool,
            scratch.groups(),
            "dead-pool-feat",
            None,
        )
        .await
        .expect_err("read must reject before touching the dead pool");
        assert!(
            matches!(err, MilestoneError::NotAMilestone { .. }),
            "expected NotAMilestone without any cache query, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn list_default_hides_completed_rollup() {
        let scratch = ScratchHome::new().await.expect("scratch home");
        let seeded = scratch.seed_group("milestone-group").await.expect("seed");
        let entry = scratch.groups().get(&seeded.group_id).await.expect("entry");
        let (_tmp, pool) = scratch_pool().await;

        let empty = add_milestone(
            scratch.backend(),
            &entry,
            AddSpec {
                slug: Some("empty-milestone".into()),
                title: "Empty".into(),
                ..AddSpec::default()
            },
            scratch.author(),
        )
        .await
        .expect("seed empty milestone");
        assert_eq!(empty.rollup.status, RollupStatus::Planning);

        let done = add_milestone(
            scratch.backend(),
            &entry,
            AddSpec {
                slug: Some("done-milestone".into()),
                title: "Done".into(),
                ..AddSpec::default()
            },
            scratch.author(),
        )
        .await
        .expect("seed done milestone");
        assert_eq!(done.slug, "done-milestone");

        crate::features::add_feature(
            scratch.backend(),
            &entry,
            crate::features::AddSpec {
                slug: Some("done-feat".into()),
                title: "feat".into(),
                description: "rollup test".into(),
                body: "x".into(),
                status: mmcp_core::memory::FeatureStatus::Completed,
                milestone: Some(
                    resolve_memory(
                        scratch.backend(),
                        &entry.handle,
                        Some("done-milestone"),
                        None,
                    )
                    .await
                    .expect("resolve done milestone")
                    .id,
                ),
                ..crate::features::AddSpec::default()
            },
            scratch.author(),
        )
        .await
        .expect("seed completed feature");

        let (visible, findings) =
            list_milestones(scratch.backend(), &entry, &pool, scratch.groups(), false)
                .await
                .expect("list default");
        assert!(findings.is_empty());
        let slugs: Vec<&str> = visible.iter().map(|r| r.slug.as_str()).collect();
        assert!(slugs.contains(&"empty-milestone"));
        assert!(
            !slugs.contains(&"done-milestone"),
            "a fully-completed rollup must hide by default: {slugs:?}"
        );

        let (everything, _findings) =
            list_milestones(scratch.backend(), &entry, &pool, scratch.groups(), true)
                .await
                .expect("list all");
        let all_slugs: Vec<&str> = everything.iter().map(|r| r.slug.as_str()).collect();
        assert!(all_slugs.contains(&"done-milestone"));
        let done_record = everything
            .iter()
            .find(|r| r.slug == "done-milestone")
            .expect("done milestone present");
        assert_eq!(done_record.rollup.status, RollupStatus::Completed);
        assert_eq!(done_record.rollup.counted, 1);
    }

    #[tokio::test]
    async fn cross_group_feature_does_not_influence_milestone_rollup() {
        // Security regression: a feature filed in an unrelated group and pointed at this milestone
        // must NOT count toward the rollup.
        // The rollup scope is limited to the milestone's own group:
        // a cross-group fold would let any caller with write access to any unprotected group
        // inject a status into a victim milestone without ever touching the victim's group.
        let scratch = ScratchHome::new().await.expect("scratch home");
        let milestone_group = scratch
            .seed_group("milestone-owner-group")
            .await
            .expect("seed milestone group");
        let foreign_group = scratch
            .seed_group("foreign-group")
            .await
            .expect("seed foreign group");
        let milestone_entry = scratch
            .groups()
            .get(&milestone_group.group_id)
            .await
            .expect("milestone entry");
        let foreign_entry = scratch
            .groups()
            .get(&foreign_group.group_id)
            .await
            .expect("foreign entry");
        let (_tmp, pool) = scratch_pool().await;

        add_milestone(
            scratch.backend(),
            &milestone_entry,
            AddSpec {
                slug: Some("cross-group-milestone".into()),
                title: "Cross Group Milestone".into(),
                ..AddSpec::default()
            },
            scratch.author(),
        )
        .await
        .expect("seed milestone");
        let milestone_id = resolve_memory(
            scratch.backend(),
            &milestone_entry.handle,
            Some("cross-group-milestone"),
            None,
        )
        .await
        .expect("resolve milestone")
        .id;

        crate::features::add_feature(
            scratch.backend(),
            &foreign_entry,
            crate::features::AddSpec {
                slug: Some("foreign-group-feat".into()),
                title: "feat filed in a foreign group".into(),
                description: "cross group rollup injection attempt".into(),
                body: "x".into(),
                status: mmcp_core::memory::FeatureStatus::Blocked,
                milestone: Some(milestone_id),
                ..crate::features::AddSpec::default()
            },
            scratch.author(),
        )
        .await
        .expect("seed feature in foreign group");

        let record = read_milestone(
            scratch.backend(),
            &milestone_entry,
            &pool,
            scratch.groups(),
            "cross-group-milestone",
            None,
        )
        .await
        .expect("read milestone");
        assert_eq!(
            record.rollup.status,
            RollupStatus::Planning,
            "a feature filed in a foreign group must not count: {:?}",
            record.rollup
        );
        assert_eq!(record.rollup.counted, 0);
    }

    #[tokio::test]
    async fn mixed_status_features_in_the_milestones_own_group_fold_into_in_progress() {
        // Exercises the fold rule (Completed + Pending -> InProgress,
        // then a flip to Blocked -> Blocked) with both features in
        // the milestone's OWN group, matching the narrowed rollup
        // scope; a same-shape sibling test above proves a foreign
        // group's feature is excluded rather than folded in here.
        let scratch = ScratchHome::new().await.expect("scratch home");
        let milestone_group = scratch
            .seed_group("gate-milestone-group")
            .await
            .expect("seed milestone group");
        let milestone_entry = scratch
            .groups()
            .get(&milestone_group.group_id)
            .await
            .expect("milestone entry");
        let (_tmp, pool) = scratch_pool().await;

        add_milestone(
            scratch.backend(),
            &milestone_entry,
            AddSpec {
                slug: Some("gate-milestone".into()),
                title: "Gate Milestone".into(),
                ..AddSpec::default()
            },
            scratch.author(),
        )
        .await
        .expect("seed milestone");
        let milestone_id = resolve_memory(
            scratch.backend(),
            &milestone_entry.handle,
            Some("gate-milestone"),
            None,
        )
        .await
        .expect("resolve milestone")
        .id;

        // One Completed feature.
        crate::features::add_feature(
            scratch.backend(),
            &milestone_entry,
            crate::features::AddSpec {
                slug: Some("feat-a-done".into()),
                title: "done".into(),
                description: "gate test".into(),
                body: "x".into(),
                status: mmcp_core::memory::FeatureStatus::Completed,
                milestone: Some(milestone_id),
                ..crate::features::AddSpec::default()
            },
            scratch.author(),
        )
        .await
        .expect("seed completed feature");

        // One still-in-progress feature.
        crate::features::add_feature(
            scratch.backend(),
            &milestone_entry,
            crate::features::AddSpec {
                slug: Some("feat-b-pending".into()),
                title: "pending".into(),
                description: "gate test".into(),
                body: "x".into(),
                status: mmcp_core::memory::FeatureStatus::Pending,
                milestone: Some(milestone_id),
                ..crate::features::AddSpec::default()
            },
            scratch.author(),
        )
        .await
        .expect("seed pending feature");

        let record = read_milestone(
            scratch.backend(),
            &milestone_entry,
            &pool,
            scratch.groups(),
            "gate-milestone",
            None,
        )
        .await
        .expect("read milestone");
        assert_eq!(
            record.rollup.status,
            RollupStatus::InProgress,
            "a completed feature plus a pending feature must fold into InProgress: {:?}",
            record.rollup
        );
        assert_eq!(record.rollup.counted, 2, "both features must count");
        assert_eq!(record.rollup.completed, 1);

        // Now flip the pending feature to Blocked and confirm the
        // rollup updates to reflect the mixed set correctly again.
        crate::features::update_feature(
            scratch.backend(),
            &milestone_entry,
            "feat-b-pending",
            crate::features::UpdateSpec {
                status: Some(mmcp_core::memory::FeatureStatus::Blocked),
                ..crate::features::UpdateSpec::default()
            },
            scratch.author(),
        )
        .await
        .expect("flip pending feature to blocked");
        // This test opens its OWN cache pool (`scratch_pool`) rather than the process-global one
        // `cache::notify_write` pushes incremental updates through (see `cache::mod`'s doc on `ACTIVE_POOL`),
        // so `ensure_built` alone would keep reading the snapshot from the first `read_milestone` call above.
        // A real CLI/MCP process wires the write-trigger hook to the SAME pool it queries,
        // so this manual rebuild only stands in for that already-tested live-update path;
        // the fold logic under test is identical either way.
        crate::cache::rebuild_full(&pool, scratch.backend(), scratch.groups())
            .await
            .expect("rebuild cache after status flip");

        let record = read_milestone(
            scratch.backend(),
            &milestone_entry,
            &pool,
            scratch.groups(),
            "gate-milestone",
            None,
        )
        .await
        .expect("read milestone after status flip");
        assert_eq!(
            record.rollup.status,
            RollupStatus::Blocked,
            "the newly blocked feature must win the fold: {:?}",
            record.rollup
        );
        assert_eq!(record.rollup.blocked, 1);
        assert_eq!(record.rollup.counted, 2);
    }
}
