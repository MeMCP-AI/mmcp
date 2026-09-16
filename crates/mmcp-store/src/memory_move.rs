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
/// write lands first, then the source delete. A crash between the
/// two leaves the memory readable in both groups, a duplicate the
/// next attempt's id-collision check catches; the reverse order
/// would risk losing the memory entirely if the target write then
/// failed.
///
/// Refuses with [`ImportError::CrossGroupIdCollision`] when the
/// target group already holds a memory with this id, and with
/// [`ImportError::TrackerNumberCollision`] when the moving memory
/// carries a tracker `number` already allocated in the target group,
/// unless `options.renumber` is set.
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

    reject_target_id_collision(
        backend,
        target_entry,
        resolved.id,
        source_group,
        target_group,
    )
    .await?;

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

    let source_commit_id = crate::memory::delete_file_at_path(
        backend,
        &source_entry.handle,
        &resolved.path,
        author,
        Some(&commit_message),
    )
    .await?;

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

/// Refuse the move when `id` already resolves to a memory in the
/// target group, naming both groups in the typed error.
async fn reject_target_id_collision(
    backend: &NativeBackend,
    target_entry: &GroupEntry,
    id: Uuid,
    source_group: Uuid,
    target_group: Uuid,
) -> Result<(), ImportError> {
    match resolve_memory(backend, &target_entry.handle, None, Some(id)).await {
        Ok(_) => Err(ImportError::CrossGroupIdCollision {
            id,
            source_group,
            target_group,
        }),
        Err(ImportError::MemoryNotFound { .. }) => Ok(()),
        Err(other) => Err(other),
    }
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
    let Some(carried_number) = file
        .frontmatter
        .feature
        .as_ref()
        .and_then(|meta| meta.number)
        .or_else(|| file.frontmatter.issue.as_ref().and_then(|meta| meta.number))
    else {
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
    }
}
