//! Cross-group memory move: the memory keeps its id and body, moves
//! into a different group's repository, and carries a typed pointer
//! back to its pre-move history (see
//! [`mmcp_core::memory::CrossGroupHistoryPointer`]).
//!
//! Distinct from [`crate::memory::move_memory_path`], which renames a
//! memory's slug inside the SAME group's repository: that primitive
//! preserves the file byte-for-byte because the frontmatter never
//! changes. A cross-group move always re-renders the frontmatter (the
//! history pointer is new content), so the verbatim contract only
//! covers the body.
//!
//! Locking is the caller's responsibility, same convention as
//! [`crate::memory::move_memory_path`]: acquire
//! [`crate::lock::cross_group_move_chain`] before calling.

use mmcp_core::id::MemoryId;
use mmcp_core::memory::{CrossGroupHistoryPointer, MemoryFile};
use mmcp_git::{CommitSpec, GitBackend, GitError, NativeBackend, Rev};
use uuid::Uuid;

use crate::groups::GroupEntry;
use crate::history::walk_path_history;
use crate::home::ResolvedAuthor;
use crate::memory::{ImportError, resolve_commit_message, resolve_memory};
use crate::tracker;

/// Caller-supplied knobs for [`move_memory_across_groups`].
#[derive(Debug, Clone, Copy, Default)]
pub struct CrossGroupMoveOptions<'a> {
    /// When the moving memory carries a tracker `number` that already
    /// collides with one allocated in the target group, mint a fresh
    /// number there instead of refusing the move.
    pub renumber: bool,
    /// Override the commit message on both the target write and the
    /// source delete. Bounded via [`resolve_commit_message`].
    pub message: Option<&'a str>,
}

/// Outcome of a successful [`move_memory_across_groups`] call.
#[derive(Debug, Clone)]
pub struct CrossGroupMoveOutcome {
    pub id: Uuid,
    pub slug: String,
    pub source_group: Uuid,
    pub target_group: Uuid,
    /// Commit that wrote the memory into the target group.
    pub target_commit_id: String,
    /// Commit that deleted the memory from the source group.
    pub source_commit_id: String,
    /// `Some((old, new))` when a tracker number collision was
    /// resolved by minting a fresh number in the target group.
    pub renumbered: Option<(u32, u32)>,
}

/// Move a memory from `source_entry`'s group to `target_entry`'s
/// group, keeping its id and body.
///
/// Order of operations, deliberate for crash safety: the target
/// write lands first, then the source delete. A crash, or any other
/// failure, between the two leaves the memory readable in both
/// groups; see [`ImportError::CrossGroupMoveHalfCompleted`] for how a
/// retry resumes from there instead of refusing on an id collision.
/// The reverse order would risk losing the memory entirely if the
/// target write then failed.
///
/// Refuses with [`ImportError::CrossGroupIdCollision`] when the
/// target group already holds an UNRELATED memory with this id, and
/// with [`ImportError::TrackerNumberCollision`] when the moving
/// memory carries a tracker `number` already allocated in the target
/// group, unless `options.renumber` is set.
pub async fn move_memory_across_groups(
    backend: &NativeBackend,
    source_entry: &GroupEntry,
    target_entry: &GroupEntry,
    slug: Option<&str>,
    id: Option<Uuid>,
    author: &ResolvedAuthor,
    options: CrossGroupMoveOptions<'_>,
) -> Result<CrossGroupMoveOutcome, ImportError> {
    let source_group = source_entry.handle.group_id;
    let target_group = target_entry.handle.group_id;

    let resolved = resolve_memory(backend, &source_entry.handle, slug, id).await?;

    match reject_target_id_collision(
        backend,
        target_entry,
        resolved.id,
        source_group,
        &resolved.path,
        target_group,
    )
    .await?
    {
        TargetIdState::Clear => {}
        TargetIdState::Resume(target_file) => {
            return resume_half_completed_move(
                backend,
                source_entry,
                target_entry,
                &resolved,
                *target_file,
                author,
                options.message,
            )
            .await;
        }
    }

    let bytes = backend
        .read_file(&source_entry.handle, &resolved.path, &Rev::head())
        .await?;
    let text = std::str::from_utf8(&bytes).map_err(|source| ImportError::NotUtf8 {
        path: resolved.path.clone(),
        source,
    })?;
    let mut file = MemoryFile::parse(text)?;

    let renumbered = reconcile_tracker_number(
        backend,
        target_entry,
        &mut file,
        options.renumber,
        source_group,
        target_group,
    )
    .await?;

    let history = walk_path_history(backend, &source_entry.handle, &resolved.path, None).await?;
    let pointer = build_history_pointer(source_group, &resolved.path, &history)?;
    file.frontmatter.history_source = Some(pointer);

    let rendered = file
        .to_string()
        .map_err(|e| ImportError::Render(e.to_string()))?;
    // The target write below bypasses `write_file_at_path` (it
    // addresses a different group's handle and must not run that
    // primitive's id-mismatch/ceiling checks meant for a same-group
    // caller), so the length validation it would otherwise provide is
    // run explicitly here instead of silently skipped.
    crate::memory::validate_write_content_lengths(&rendered)?;

    let target_path =
        mmcp_core::conventions::memory_path(&resolved.slug, MemoryId::from_uuid(resolved.id));
    let commit_message = resolve_commit_message(options.message, || {
        format!(
            "move memory {} from group {source_group} to group {target_group}",
            resolved.slug
        )
    })?;
    let target_commit_id = backend
        .write_commit(
            &target_entry.handle,
            CommitSpec::mmcp_commit(
                commit_message.clone(),
                vec![(target_path.clone(), Some(rendered.clone().into_bytes()))],
                &author.name,
                &author.email,
            ),
        )
        .await?;

    let source_commit_id = match crate::memory::delete_file_at_path(
        backend,
        &source_entry.handle,
        &resolved.path,
        author,
        Some(&commit_message),
    )
    .await
    {
        Ok(commit_id) => commit_id,
        Err(err) => {
            return Err(half_completed(
                resolved.id,
                &resolved.slug,
                source_group,
                target_group,
                &target_path,
                target_commit_id,
                &rendered,
                err,
            )
            .await);
        }
    };

    crate::cache::notify_move(
        source_group,
        target_group,
        resolved.id,
        &resolved.slug,
        &target_path,
        &target_commit_id,
        &rendered,
    )
    .await;

    Ok(CrossGroupMoveOutcome {
        id: resolved.id,
        slug: resolved.slug,
        source_group,
        target_group,
        target_commit_id,
        source_commit_id,
        renumbered,
    })
}

/// Build [`ImportError::CrossGroupMoveHalfCompleted`] after a source
/// delete failure, first updating the target group's cache row to the
/// already-landed target write (best-effort, same as any other write)
/// so a caller inspecting the cache before retrying sees the moved
/// copy is already there.
#[allow(clippy::too_many_arguments)]
async fn half_completed(
    id: Uuid,
    slug: &str,
    source_group: Uuid,
    target_group: Uuid,
    target_path: &str,
    target_commit_id: String,
    rendered: &str,
    source: ImportError,
) -> ImportError {
    crate::cache::notify_write(
        target_group,
        id,
        slug,
        target_path,
        &target_commit_id,
        rendered,
    )
    .await;
    ImportError::CrossGroupMoveHalfCompleted(Box::new(
        crate::memory::CrossGroupMoveHalfCompletedDetail {
            id,
            source_group,
            target_group,
            target_commit_id,
            source,
        },
    ))
}

/// Outcome of [`reject_target_id_collision`]'s check against the
/// target group.
enum TargetIdState {
    /// No memory in the target group carries this id: the move is
    /// free to write there.
    Clear,
    /// The target group already holds a memory with this id, and its
    /// own `history_source` names this exact source group and path:
    /// a prior attempt's target write already landed
    /// (see [`ImportError::CrossGroupMoveHalfCompleted`]), so only
    /// the source delete and the cache transition remain.
    Resume(Box<MemoryFile>),
}

/// Check whether `id` already resolves to a memory in the target
/// group. A collision whose target copy's `history_source` names
/// `source_path` in `source_group` is a resumable half-completed
/// move, not a genuine collision; any other existing copy refuses
/// with [`ImportError::CrossGroupIdCollision`], naming both groups.
async fn reject_target_id_collision(
    backend: &NativeBackend,
    target_entry: &GroupEntry,
    id: Uuid,
    source_group: Uuid,
    source_path: &str,
    target_group: Uuid,
) -> Result<TargetIdState, ImportError> {
    let existing = match resolve_memory(backend, &target_entry.handle, None, Some(id)).await {
        Ok(existing) => existing,
        Err(ImportError::MemoryNotFound { .. }) => return Ok(TargetIdState::Clear),
        Err(other) => return Err(other),
    };

    let bytes = backend
        .read_file(&target_entry.handle, &existing.path, &Rev::head())
        .await?;
    let text = std::str::from_utf8(&bytes).map_err(|source| ImportError::NotUtf8 {
        path: existing.path.clone(),
        source,
    })?;
    let target_file = MemoryFile::parse(text)?;

    let is_resume = target_file
        .frontmatter
        .history_source
        .as_ref()
        .is_some_and(|pointer| {
            pointer.source_group == source_group && pointer.source_path == source_path
        });

    if is_resume {
        Ok(TargetIdState::Resume(Box::new(target_file)))
    } else {
        Err(ImportError::CrossGroupIdCollision {
            id,
            source_group,
            target_group,
        })
    }
}

/// Complete a previously half-completed move: `target_file` is the
/// target copy's already-committed content, its `history_source`
/// already confirmed by [`reject_target_id_collision`] to name this
/// exact source group and path, so only the source delete and the
/// cache transition remain.
async fn resume_half_completed_move(
    backend: &NativeBackend,
    source_entry: &GroupEntry,
    target_entry: &GroupEntry,
    resolved: &crate::memory::ResolvedMemory,
    target_file: MemoryFile,
    author: &ResolvedAuthor,
    message: Option<&str>,
) -> Result<CrossGroupMoveOutcome, ImportError> {
    let source_group = source_entry.handle.group_id;
    let target_group = target_entry.handle.group_id;
    let target_path =
        mmcp_core::conventions::memory_path(&resolved.slug, MemoryId::from_uuid(resolved.id));

    // The prior attempt's own commit id: the most recent (and, since
    // the target write is a single commit, only) entry touching this
    // path in the target group.
    let target_commit_id = walk_path_history(backend, &target_entry.handle, &target_path, Some(1))
        .await?
        .into_iter()
        .next()
        .map(|commit| commit.id)
        .ok_or_else(|| ImportError::Git(GitError::PathNotFound(target_path.clone())))?;

    // The source copy's carried tracker number, read before the
    // delete below removes it, so a resumed move still reports an
    // earlier attempt's renumber accurately.
    let source_bytes = backend
        .read_file(&source_entry.handle, &resolved.path, &Rev::head())
        .await?;
    let source_text =
        std::str::from_utf8(&source_bytes).map_err(|source| ImportError::NotUtf8 {
            path: resolved.path.clone(),
            source,
        })?;
    let source_file = MemoryFile::parse(source_text)?;
    let renumbered = match (
        carried_ticket_number(&source_file),
        carried_ticket_number(&target_file),
    ) {
        (Some(old), Some(new)) if old != new => Some((old, new)),
        _ => None,
    };

    let commit_message = resolve_commit_message(message, || {
        format!(
            "move memory {} from group {source_group} to group {target_group}",
            resolved.slug
        )
    })?;

    let rendered = target_file
        .to_string()
        .map_err(|e| ImportError::Render(e.to_string()))?;

    let source_commit_id = match crate::memory::delete_file_at_path(
        backend,
        &source_entry.handle,
        &resolved.path,
        author,
        Some(&commit_message),
    )
    .await
    {
        Ok(commit_id) => commit_id,
        Err(err) => {
            return Err(half_completed(
                resolved.id,
                &resolved.slug,
                source_group,
                target_group,
                &target_path,
                target_commit_id,
                &rendered,
                err,
            )
            .await);
        }
    };

    crate::cache::notify_move(
        source_group,
        target_group,
        resolved.id,
        &resolved.slug,
        &target_path,
        &target_commit_id,
        &rendered,
    )
    .await;

    Ok(CrossGroupMoveOutcome {
        id: resolved.id,
        slug: resolved.slug.clone(),
        source_group,
        target_group,
        target_commit_id,
        source_commit_id,
        renumbered,
    })
}

/// A hybrid memory's carried tracker number: `feature.number`,
/// falling back to `issue.number` (mirrors the fold
/// [`tracker::next_ticket_number`] performs when minting one).
fn carried_ticket_number(file: &MemoryFile) -> Option<u32> {
    file.frontmatter
        .feature
        .as_ref()
        .and_then(|meta| meta.number)
        .or_else(|| file.frontmatter.issue.as_ref().and_then(|meta| meta.number))
}

/// A hybrid memory shares one ticket number across its `feature` and
/// `issue` blocks (see [`tracker::next_ticket_number`]): read it once,
/// check it once against the target group, and either refuse or
/// write the resolved value back onto whichever blocks are present.
///
/// Returns `Some((old, new))` when `renumber` minted a fresh number
/// to resolve a collision, `None` when the memory carries no tracker
/// number or its number did not collide.
async fn reconcile_tracker_number(
    backend: &NativeBackend,
    target_entry: &GroupEntry,
    file: &mut MemoryFile,
    renumber: bool,
    source_group: Uuid,
    target_group: Uuid,
) -> Result<Option<(u32, u32)>, ImportError> {
    let Some(carried_number) = carried_ticket_number(file) else {
        return Ok(None);
    };

    if !tracker::number_collision(backend, target_entry, carried_number).await? {
        return Ok(None);
    }

    if !renumber {
        return Err(ImportError::TrackerNumberCollision {
            kind: file.frontmatter.kind.as_str(),
            number: carried_number,
            source_group,
            target_group,
        });
    }

    let new_number = tracker::next_ticket_number(backend, target_entry).await?;
    if let Some(meta) = file.frontmatter.feature.as_mut() {
        meta.number = Some(new_number);
    }
    if let Some(meta) = file.frontmatter.issue.as_mut() {
        meta.number = Some(new_number);
    }
    Ok(Some((carried_number, new_number)))
}

/// Build the [`CrossGroupHistoryPointer`] recorded on the moved
/// memory's frontmatter: `history` is the source group's walk over
/// `path` before the move's delete commit, most recent first.
fn build_history_pointer(
    source_group: Uuid,
    path: &str,
    history: &[mmcp_git::CommitMeta],
) -> Result<CrossGroupHistoryPointer, ImportError> {
    let last_commit = history
        .first()
        .map(|commit| commit.id.clone())
        .ok_or_else(|| ImportError::Git(GitError::PathNotFound(path.to_string())))?;
    let first_commit = history
        .last()
        .map(|commit| commit.id.clone())
        .unwrap_or_else(|| last_commit.clone());
    CrossGroupHistoryPointer::new(source_group, path, first_commit, last_commit)
        .map_err(|err| ImportError::Render(err.to_string()))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use crate::memory::import_memory;
    use crate::testing::{ScratchHome, read_current_frontmatter};
    use mmcp_core::memory::{
        FeatureMetadata, FeatureStatus, FrontmatterFormat, MemoryFrontmatter, MemoryKind,
    };

    fn feature_body(name: &str, number: u32) -> MemoryFile {
        MemoryFile {
            frontmatter: MemoryFrontmatter::new(name, "a moving feature", MemoryKind::Feature)
                .with_feature(FeatureMetadata {
                    status: FeatureStatus::Requested,
                    number: Some(number),
                    depends_on: vec![Uuid::now_v7()],
                    blocks: vec![Uuid::now_v7()],
                    milestone: Some(Uuid::now_v7()),
                    ..FeatureMetadata::default()
                }),
            body: format!("## Need\n\n{name} needs to move.\n"),
            format: FrontmatterFormat::TomlPlus,
        }
    }

    async fn two_groups(scratch: &ScratchHome) -> (GroupEntry, GroupEntry) {
        let source = scratch
            .seed_group("move-source")
            .await
            .expect("seed source");
        let target = scratch
            .seed_group("move-target")
            .await
            .expect("seed target");
        let source_entry = scratch
            .groups()
            .get(&source.group_id)
            .await
            .expect("source entry");
        let target_entry = scratch
            .groups()
            .get(&target.group_id)
            .await
            .expect("target entry");
        (source_entry, target_entry)
    }

    #[tokio::test]
    async fn move_preserves_id_and_body_verbatim() {
        let scratch = ScratchHome::new().await.expect("scratch home");
        let (source_entry, target_entry) = two_groups(&scratch).await;

        let file = feature_body("movable", 1);
        let rendered = file.to_string().expect("render");
        let seeded = import_memory(
            scratch.backend(),
            &source_entry.handle,
            "movable",
            &rendered,
            None,
            scratch.author(),
            false,
        )
        .await
        .expect("seed source memory");

        let source_path =
            mmcp_core::conventions::memory_path("movable", MemoryId::from_uuid(seeded.id));
        let source_bytes_before =
            crate::testing::read_raw_bytes(scratch.backend(), &source_entry.handle, &source_path)
                .await
                .expect("read source bytes before move");
        let source_body_before =
            MemoryFile::parse(&String::from_utf8(source_bytes_before).expect("utf8"))
                .expect("parse")
                .body;

        let outcome = move_memory_across_groups(
            scratch.backend(),
            &source_entry,
            &target_entry,
            None,
            Some(seeded.id),
            scratch.author(),
            CrossGroupMoveOptions::default(),
        )
        .await
        .expect("move across groups");

        assert_eq!(outcome.id, seeded.id);
        assert_eq!(outcome.renumbered, None);

        // Body byte-equal in the target; the id resolves there now.
        let target_frontmatter =
            read_current_frontmatter(scratch.backend(), &target_entry.handle, "movable")
                .await
                .expect("read target frontmatter");
        assert_eq!(target_frontmatter.id, Some(seeded.id));
        let target_bytes = crate::testing::read_raw_bytes(
            scratch.backend(),
            &target_entry.handle,
            &mmcp_core::conventions::memory_path("movable", MemoryId::from_uuid(seeded.id)),
        )
        .await
        .expect("read target bytes");
        let target_text = String::from_utf8(target_bytes).expect("utf8");
        let target_file = MemoryFile::parse(&target_text).expect("parse target file");
        assert_eq!(target_file.body, source_body_before);

        // Gone from the source.
        let source_lookup = resolve_memory(
            scratch.backend(),
            &source_entry.handle,
            Some("movable"),
            None,
        )
        .await;
        assert!(source_lookup.is_err(), "source must no longer resolve");

        // The pointer names the source group and a valid commit range.
        let pointer = target_frontmatter
            .history_source
            .expect("history pointer set");
        assert_eq!(pointer.source_group, source_entry.handle.group_id);
    }

    #[tokio::test]
    async fn move_refuses_on_target_id_collision() {
        let scratch = ScratchHome::new().await.expect("scratch home");
        let (source_entry, target_entry) = two_groups(&scratch).await;

        let file = feature_body("clashing", 1);
        let rendered = file.to_string().expect("render");
        let seeded = import_memory(
            scratch.backend(),
            &source_entry.handle,
            "clashing",
            &rendered,
            None,
            scratch.author(),
            false,
        )
        .await
        .expect("seed source memory");

        // Pre-seed the SAME id in the target group directly (bypassing
        // the normal id-minting path) so the collision is deterministic.
        let mut colliding = feature_body("already-there", 99);
        colliding.frontmatter = colliding.frontmatter.with_id(seeded.id);
        import_memory(
            scratch.backend(),
            &target_entry.handle,
            "already-there",
            &colliding.to_string().expect("render colliding"),
            None,
            scratch.author(),
            false,
        )
        .await
        .expect("seed target collision");

        let err = move_memory_across_groups(
            scratch.backend(),
            &source_entry,
            &target_entry,
            None,
            Some(seeded.id),
            scratch.author(),
            CrossGroupMoveOptions::default(),
        )
        .await
        .expect_err("id collision must refuse the move");

        match err {
            ImportError::CrossGroupIdCollision {
                id,
                source_group,
                target_group,
            } => {
                assert_eq!(id, seeded.id);
                assert_eq!(source_group, source_entry.handle.group_id);
                assert_eq!(target_group, target_entry.handle.group_id);
            }
            other => panic!("expected CrossGroupIdCollision, got {other:?}"),
        }

        // The source memory must still resolve: a refused move changes nothing.
        let still_there = resolve_memory(
            scratch.backend(),
            &source_entry.handle,
            Some("clashing"),
            None,
        )
        .await;
        assert!(
            still_there.is_ok(),
            "a refused move must leave the source untouched"
        );
    }

    #[tokio::test]
    async fn move_refuses_on_tracker_number_collision_naming_both_groups() {
        let scratch = ScratchHome::new().await.expect("scratch home");
        let (source_entry, target_entry) = two_groups(&scratch).await;

        let file = feature_body("carries-seven", 7);
        let rendered = file.to_string().expect("render");
        let seeded = import_memory(
            scratch.backend(),
            &source_entry.handle,
            "carries-seven",
            &rendered,
            None,
            scratch.author(),
            false,
        )
        .await
        .expect("seed source memory");

        let target_taken = feature_body("target-already-seven", 7);
        import_memory(
            scratch.backend(),
            &target_entry.handle,
            "target-already-seven",
            &target_taken.to_string().expect("render target taken"),
            None,
            scratch.author(),
            false,
        )
        .await
        .expect("seed target number 7");

        let err = move_memory_across_groups(
            scratch.backend(),
            &source_entry,
            &target_entry,
            None,
            Some(seeded.id),
            scratch.author(),
            CrossGroupMoveOptions::default(),
        )
        .await
        .expect_err("number collision must refuse without renumber");

        match err {
            ImportError::TrackerNumberCollision {
                kind,
                number,
                source_group,
                target_group,
            } => {
                assert_eq!(kind, "feature");
                assert_eq!(number, 7);
                assert_eq!(source_group, source_entry.handle.group_id);
                assert_eq!(target_group, target_entry.handle.group_id);
            }
            other => panic!("expected TrackerNumberCollision, got {other:?}"),
        }

        // With renumber, the move succeeds and reports old and new.
        let depends_on = file
            .frontmatter
            .feature
            .as_ref()
            .unwrap()
            .depends_on
            .clone();
        let blocks = file.frontmatter.feature.as_ref().unwrap().blocks.clone();
        let milestone = file.frontmatter.feature.as_ref().unwrap().milestone;
        let outcome = move_memory_across_groups(
            scratch.backend(),
            &source_entry,
            &target_entry,
            None,
            Some(seeded.id),
            scratch.author(),
            CrossGroupMoveOptions {
                renumber: true,
                message: None,
            },
        )
        .await
        .expect("renumbered move must succeed");

        let (old_number, new_number) = outcome.renumbered.expect("renumber reported");
        assert_eq!(old_number, 7);
        assert_ne!(new_number, 7);

        let target_frontmatter =
            read_current_frontmatter(scratch.backend(), &target_entry.handle, "carries-seven")
                .await
                .expect("read moved frontmatter");
        let moved_feature = target_frontmatter.feature.expect("feature block carried");
        assert_eq!(moved_feature.number, Some(new_number));
        assert_eq!(moved_feature.depends_on, depends_on);
        assert_eq!(moved_feature.blocks, blocks);
        assert_eq!(moved_feature.milestone, milestone);
    }

    #[tokio::test]
    async fn move_resumes_a_half_completed_move_when_the_target_already_carries_the_pointer() {
        let scratch = ScratchHome::new().await.expect("scratch home");
        let (source_entry, target_entry) = two_groups(&scratch).await;

        let file = feature_body("resumable", 1);
        let rendered = file.to_string().expect("render");
        let seeded = import_memory(
            scratch.backend(),
            &source_entry.handle,
            "resumable",
            &rendered,
            None,
            scratch.author(),
            false,
        )
        .await
        .expect("seed source memory");

        // Hand-construct the state a crashed first attempt would have
        // left behind: the target write landed (with the history
        // pointer already set), but the source delete never ran, so
        // the memory still resolves in BOTH groups.
        let source_path =
            mmcp_core::conventions::memory_path("resumable", MemoryId::from_uuid(seeded.id));
        let history =
            walk_path_history(scratch.backend(), &source_entry.handle, &source_path, None)
                .await
                .expect("walk source history");
        let pointer = build_history_pointer(source_entry.handle.group_id, &source_path, &history)
            .expect("build pointer");
        // Re-parse the ACTUAL post-import source content (which
        // carries the minted id `import_memory` assigned), not the
        // pre-import `rendered` string: that one never had an id set,
        // and writing it as-is would make the hand-crafted target
        // copy resolve by nothing, defeating the whole setup.
        let source_bytes =
            crate::testing::read_raw_bytes(scratch.backend(), &source_entry.handle, &source_path)
                .await
                .expect("read actual source bytes");
        let mut half_moved = MemoryFile::parse(&String::from_utf8(source_bytes).expect("utf8"))
            .expect("parse seeded content");
        half_moved.frontmatter.history_source = Some(pointer);
        let half_moved_rendered = half_moved.to_string().expect("render half-moved content");
        let prior_target_commit = scratch
            .backend()
            .write_commit(
                &target_entry.handle,
                CommitSpec::mmcp_commit(
                    "simulated first attempt: target write",
                    vec![(source_path.clone(), Some(half_moved_rendered.into_bytes()))],
                    &scratch.author().name,
                    &scratch.author().email,
                ),
            )
            .await
            .expect("simulate the prior attempt's target write");

        // Source is still there: the simulated first attempt never
        // got as far as the delete.
        let still_resolves = resolve_memory(
            scratch.backend(),
            &source_entry.handle,
            Some("resumable"),
            None,
        )
        .await;
        assert!(
            still_resolves.is_ok(),
            "source must still resolve before the resume"
        );

        // Retry the SAME move: `reject_target_id_collision` must
        // recognize the target copy's pointer and resume instead of
        // refusing on an id collision.
        let outcome = move_memory_across_groups(
            scratch.backend(),
            &source_entry,
            &target_entry,
            None,
            Some(seeded.id),
            scratch.author(),
            CrossGroupMoveOptions::default(),
        )
        .await
        .expect("a matching pointer must resume, not refuse as a collision");

        assert_eq!(outcome.id, seeded.id);
        assert_eq!(
            outcome.target_commit_id, prior_target_commit,
            "resume must report the PRIOR attempt's target commit, never mint a new one"
        );

        let source_gone = resolve_memory(
            scratch.backend(),
            &source_entry.handle,
            Some("resumable"),
            None,
        )
        .await;
        assert!(
            source_gone.is_err(),
            "resume must complete the source delete"
        );

        let target_frontmatter =
            read_current_frontmatter(scratch.backend(), &target_entry.handle, "resumable")
                .await
                .expect("read target frontmatter");
        assert_eq!(target_frontmatter.id, Some(seeded.id));
    }

    #[tokio::test]
    async fn half_completed_names_both_groups_and_chains_the_delete_failure() {
        let id = Uuid::now_v7();
        let source_group = Uuid::now_v7();
        let target_group = Uuid::now_v7();
        let injected = ImportError::MemoryNotFound {
            slug: Some("whatever".to_string()),
            id: Some(id),
        };

        let err = half_completed(
            id,
            "resumable",
            source_group,
            target_group,
            "memories/resumable/deadbeef.md",
            "a".repeat(40),
            "irrelevant rendered content",
            injected,
        )
        .await;

        match err {
            ImportError::CrossGroupMoveHalfCompleted(detail) => {
                assert_eq!(detail.id, id);
                assert_eq!(detail.source_group, source_group);
                assert_eq!(detail.target_group, target_group);
                assert_eq!(detail.target_commit_id, "a".repeat(40));
                assert!(
                    matches!(detail.source, ImportError::MemoryNotFound { .. }),
                    "the original delete failure must chain through as the source"
                );
            }
            other => panic!("expected CrossGroupMoveHalfCompleted, got {other:?}"),
        }
    }
}
