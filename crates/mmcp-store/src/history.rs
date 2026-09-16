//! Single owning primitive for walking a repository path's commit history.
//!
//! Every caller that needs raw commit history (the `list_versions` MCP tool,
//! the CLI `mmcp memory versions` command, the `debug_git_log` diagnostic tool)
//! goes through [`walk_path_history`] instead of calling [`GitBackend::walk_history`] directly,
//! so a later cross-cutting change has one call site to update.
//!
//! [`walk_memory_history`] layers cross-group-move pointer-following
//! on top, for the two callers that address an actual memory rather
//! than an arbitrary repo path (`debug_git_log` walks the latter and
//! stays on [`walk_path_history`] directly).

use mmcp_core::id::GroupId;
use mmcp_git::{CommitMeta, GitBackend, GitError, NativeBackend, RepoHandle, Rev};
use uuid::Uuid;

use crate::groups::GroupIndex;
use crate::memory::{ImportError, read_frontmatter_at};

/// Walk the commit history of `path` in `handle`'s repo, most recent first.
/// `limit` caps the number of entries returned; `None` is unbounded.
pub async fn walk_path_history(
    backend: &NativeBackend,
    handle: &RepoHandle,
    path: &str,
    limit: Option<usize>,
) -> Result<Vec<CommitMeta>, GitError> {
    backend.walk_history(handle, path, limit).await
}

/// One history entry tagged with the group whose repository actually holds the commit.
#[derive(Debug, Clone)]
pub struct OwnedHistoryEntry {
    pub commit: CommitMeta,
    pub owning_group: Uuid,
}

/// Outcome of [`walk_memory_history`].
#[derive(Debug, Clone)]
pub struct MemoryHistoryOutcome {
    /// Target group's entries first, then the source group's when a
    /// cross-group history pointer resolves.
    pub entries: Vec<OwnedHistoryEntry>,
    /// Set when the memory's frontmatter carries a cross-group
    /// history pointer that could not be resolved (the source group
    /// is not mirrored locally, or its walk failed). The caller
    /// surfaces this as a `warn`/`dangling_ref`-family note instead
    /// of failing the call: the target-only history in `entries` is
    /// still a correct, if incomplete, answer.
    pub unresolved_pointer: Option<String>,
}

/// Walk a memory's full history: `handle`'s own commits over `path`,
/// then, when `path`'s current frontmatter carries a
/// [`mmcp_core::memory::CrossGroupHistoryPointer`], the source
/// group's commits over its pre-move path.
///
/// `limit` bounds only the target group's own walk; the appended
/// source history (when the pointer resolves) is always walked in
/// full, since a moved memory's pre-move history is bounded by
/// construction (see [`crate::memory_move::move_memory_across_groups`]).
pub async fn walk_memory_history(
    backend: &NativeBackend,
    handle: &RepoHandle,
    groups: &GroupIndex,
    path: &str,
    limit: Option<usize>,
) -> Result<MemoryHistoryOutcome, ImportError> {
    let target_group = handle.group_id;
    let mut entries: Vec<OwnedHistoryEntry> = walk_path_history(backend, handle, path, limit)
        .await
        .map_err(ImportError::Git)?
        .into_iter()
        .map(|commit| OwnedHistoryEntry {
            commit,
            owning_group: target_group,
        })
        .collect();

    let Some(pointer) = read_history_pointer(backend, handle, path).await? else {
        return Ok(MemoryHistoryOutcome {
            entries,
            unresolved_pointer: None,
        });
    };

    let Some(source_entry) = groups.get(&GroupId::from_uuid(pointer.source_group)).await else {
        return Ok(MemoryHistoryOutcome {
            entries,
            unresolved_pointer: Some(format!(
                "source group {} is not mirrored locally",
                pointer.source_group
            )),
        });
    };

    match walk_path_history(backend, &source_entry.handle, &pointer.source_path, None).await {
        Ok(source_history) => {
            entries.extend(source_history.into_iter().map(|commit| OwnedHistoryEntry {
                commit,
                owning_group: pointer.source_group,
            }));
            Ok(MemoryHistoryOutcome {
                entries,
                unresolved_pointer: None,
            })
        }
        Err(err) => Ok(MemoryHistoryOutcome {
            entries,
            unresolved_pointer: Some(format!(
                "source group {} history unreadable: {err}",
                pointer.source_group
            )),
        }),
    }
}

/// Read `path`'s frontmatter at `handle`'s `HEAD` and extract its
/// cross-group history pointer, if any. `None` covers both "no
/// pointer set" and "path unreadable": the caller treats a missing
/// pointer as the common case, not an error.
async fn read_history_pointer(
    backend: &NativeBackend,
    handle: &RepoHandle,
    path: &str,
) -> Result<Option<mmcp_core::memory::CrossGroupHistoryPointer>, ImportError> {
    match read_frontmatter_at(backend, handle, &Rev::head(), path).await {
        Ok(frontmatter) => Ok(frontmatter.history_source),
        Err(ImportError::Git(GitError::PathNotFound(_))) => Ok(None),
        Err(err) => Err(err),
    }
}

/// Read `path` at `rev` from `handle`'s repo. When `rev` names a
/// commit `handle`'s repo does not have (a `RevNotFound`) and the
/// memory's current frontmatter carries a cross-group history
/// pointer, retry against the source group's repo and its pre-move
/// path: routes a moved memory's old-history reads to the
/// repository that actually holds the commit, transparent to the
/// caller.
pub async fn read_file_following_history_pointer(
    backend: &NativeBackend,
    handle: &RepoHandle,
    groups: &GroupIndex,
    path: &str,
    rev: &Rev,
) -> Result<bytes::Bytes, ImportError> {
    match backend.read_file(handle, path, rev).await {
        Ok(bytes) => Ok(bytes),
        // A commit sha this repo's object database does not have
        // surfaces as `RevNotFound` (unparseable hex) or `ResolveRev`
        // (parseable hex, missing object): both mean "not in this
        // repo", the shape a cross-group commit produces here since
        // the target and source repositories share no git objects.
        Err(err @ (GitError::RevNotFound(_) | GitError::ResolveRev { .. }))
            if matches!(rev, Rev::Commit(_)) =>
        {
            let Some(pointer) = read_history_pointer(backend, handle, path).await? else {
                return Err(ImportError::Git(err));
            };
            let Some(source_entry) = groups.get(&GroupId::from_uuid(pointer.source_group)).await
            else {
                return Err(ImportError::Git(err));
            };
            backend
                .read_file(&source_entry.handle, &pointer.source_path, rev)
                .await
                .map_err(ImportError::Git)
        }
        Err(err) => Err(ImportError::Git(err)),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use crate::memory::{import_memory, resolve_memory};
    use crate::memory_move::{CrossGroupMoveOptions, move_memory_across_groups};
    use crate::testing::ScratchHome;
    use mmcp_core::memory::{FrontmatterFormat, MemoryFile, MemoryFrontmatter, MemoryKind};

    #[tokio::test]
    async fn walk_memory_history_appends_source_entries_after_a_real_move() {
        let scratch = ScratchHome::new().await.expect("scratch home");
        let source = scratch
            .seed_group("history-source")
            .await
            .expect("seed source");
        let target = scratch
            .seed_group("history-target")
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

        let file = MemoryFile {
            frontmatter: MemoryFrontmatter::new("moved", "carries history", MemoryKind::Scratch),
            body: "first version\n".to_string(),
            format: FrontmatterFormat::TomlPlus,
        };
        let seeded = import_memory(
            scratch.backend(),
            &source_entry.handle,
            "moved",
            &file.to_string().expect("render"),
            None,
            scratch.author(),
            false,
        )
        .await
        .expect("seed source memory");

        move_memory_across_groups(
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

        let resolved = resolve_memory(scratch.backend(), &target_entry.handle, Some("moved"), None)
            .await
            .expect("resolve moved memory in target");
        let outcome = walk_memory_history(
            scratch.backend(),
            &target_entry.handle,
            scratch.groups(),
            &resolved.path,
            None,
        )
        .await
        .expect("walk memory history");

        assert!(outcome.unresolved_pointer.is_none());
        assert_eq!(
            outcome.entries[0].owning_group,
            *target.group_id.as_uuid(),
            "the target's own write commit must come first"
        );
        assert!(
            outcome
                .entries
                .iter()
                .skip(1)
                .all(|entry| entry.owning_group == *source.group_id.as_uuid()),
            "every entry after the target's own history must be tagged with the source group"
        );
        assert!(
            outcome.entries.len() >= 2,
            "the target write plus at least the source's creation commit"
        );
    }

    #[tokio::test]
    async fn read_file_following_history_pointer_routes_a_source_commit_to_the_source_repo() {
        let scratch = ScratchHome::new().await.expect("scratch home");
        let source = scratch
            .seed_group("route-source")
            .await
            .expect("seed source");
        let target = scratch
            .seed_group("route-target")
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

        let file = MemoryFile {
            frontmatter: MemoryFrontmatter::new("routed", "carries history", MemoryKind::Scratch),
            body: "pre-move content\n".to_string(),
            format: FrontmatterFormat::TomlPlus,
        };
        let seeded = import_memory(
            scratch.backend(),
            &source_entry.handle,
            "routed",
            &file.to_string().expect("render"),
            None,
            scratch.author(),
            false,
        )
        .await
        .expect("seed source memory");
        let source_path = mmcp_core::conventions::memory_path(
            "routed",
            mmcp_core::id::MemoryId::from_uuid(seeded.id),
        );
        let expected_bytes =
            crate::testing::read_raw_bytes(scratch.backend(), &source_entry.handle, &source_path)
                .await
                .expect("read raw source bytes");

        move_memory_across_groups(
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

        let target_frontmatter = crate::testing::read_current_frontmatter(
            scratch.backend(),
            &target_entry.handle,
            "routed",
        )
        .await
        .expect("read target frontmatter");
        let pointer = target_frontmatter
            .history_source
            .expect("history pointer set by the move");

        let routed = read_file_following_history_pointer(
            scratch.backend(),
            &target_entry.handle,
            scratch.groups(),
            &source_path,
            &Rev::Commit(pointer.last_commit),
        )
        .await
        .expect("route to the source repo");
        assert_eq!(routed.as_ref(), expected_bytes.as_slice());
    }

    #[tokio::test]
    async fn walk_memory_history_degrades_to_target_only_with_a_note_when_source_is_absent() {
        let scratch = ScratchHome::new().await.expect("scratch home");
        let target = scratch
            .seed_group("dangling-target")
            .await
            .expect("seed target");
        let target_entry = scratch
            .groups()
            .get(&target.group_id)
            .await
            .expect("target entry");

        let never_mirrored_source = Uuid::now_v7();
        let pointer = mmcp_core::memory::CrossGroupHistoryPointer::new(
            never_mirrored_source,
            "memories/gone/deadbeef.md",
            "0123456789abcdef0123456789abcdef01234567",
            "fedcba9876543210fedcba9876543210fedcba98",
        )
        .expect("valid pointer shape");
        let file = MemoryFile {
            frontmatter: MemoryFrontmatter::new(
                "dangling",
                "pointer never resolves",
                MemoryKind::Scratch,
            )
            .with_history_source(Some(pointer)),
            body: "still readable locally\n".to_string(),
            format: FrontmatterFormat::TomlPlus,
        };
        let seeded = import_memory(
            scratch.backend(),
            &target_entry.handle,
            "dangling",
            &file.to_string().expect("render"),
            None,
            scratch.author(),
            false,
        )
        .await
        .expect("seed dangling memory");

        let resolved = resolve_memory(
            scratch.backend(),
            &target_entry.handle,
            Some("dangling"),
            None,
        )
        .await
        .expect("resolve dangling memory");
        let outcome = walk_memory_history(
            scratch.backend(),
            &target_entry.handle,
            scratch.groups(),
            &resolved.path,
            None,
        )
        .await
        .expect("walk memory history degrades instead of erroring");

        assert!(
            outcome.unresolved_pointer.is_some(),
            "an unresolvable pointer must surface as a warn-note reason, not an error"
        );
        assert!(
            outcome
                .entries
                .iter()
                .all(|entry| entry.owning_group == *target.group_id.as_uuid()),
            "every entry must stay target-only when the source cannot be resolved"
        );
        assert_eq!(seeded.id, resolved.id);
    }
}
