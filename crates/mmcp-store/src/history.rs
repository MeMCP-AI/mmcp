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

use std::collections::HashSet;

use mmcp_core::id::GroupId;
use mmcp_core::memory::CrossGroupHistoryPointer;
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

/// [`walk_path_history`] rooted at `root` instead of `HEAD`. Used to
/// walk a history-pointer hop: the pointed-at path no longer exists
/// at the source repo's `HEAD` once its own move-delete commit
/// landed there, so the walk has to pin the pointer's own
/// `last_commit`.
async fn walk_path_history_from(
    backend: &NativeBackend,
    handle: &RepoHandle,
    root: &Rev,
    path: &str,
    limit: Option<usize>,
) -> Result<Vec<CommitMeta>, GitError> {
    backend.walk_history_from(handle, root, path, limit).await
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
/// [`CrossGroupHistoryPointer`], every group the memory passed
/// through before landing here, one hop at a time.
///
/// Each hop reads the pointed-at file's frontmatter AT the pointer's
/// own `last_commit`, not at the source repo's `HEAD`: the path was
/// deleted there once that group's own move landed, so `HEAD` no
/// longer has it. When that frozen frontmatter itself carries a
/// further `history_source` (the memory moved more than once), the
/// walk keeps following it, tagging every group's commits with that
/// group's id, guarded against a corrupted or adversarial pointer
/// cycle by [`follow_history_chain`]'s visited set.
///
/// `limit` bounds only the target group's own walk; every appended
/// hop's history is always walked in full, since a moved memory's
/// per-group history is bounded by construction (see
/// [`crate::memory_move::move_memory_across_groups`]).
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

    let mut visited: HashSet<(Uuid, String, String)> = HashSet::new();
    let unresolved_pointer =
        follow_history_chain(backend, groups, pointer, &mut visited, &mut entries)
            .await
            .err();

    Ok(MemoryHistoryOutcome {
        entries,
        unresolved_pointer,
    })
}

/// Follow a chain of cross-group history pointers one hop at a time,
/// appending each hop's commits (tagged with its owning group) to
/// `entries`. Starts from `pointer` and keeps going as long as the
/// hop it lands on itself carries a further `history_source`.
///
/// `visited` guards against a cycle: a `(group, path, last_commit)`
/// hop identity already seen earlier in this same walk stops the
/// walk immediately, before performing any further git read, rather
/// than looping. A genuine cycle can only come from a corrupted or
/// hand-edited pointer (real moves always target a strictly earlier,
/// already-existing commit, which cannot itself reference a commit
/// that does not exist yet), so this never fires on organic move
/// history; it exists as a defensive bound on untrusted input, not
/// a limit on how many real hops a move chain may have.
///
/// Returns `Ok(())` once the chain ends normally (or is cut short by
/// the cycle guard: the entries gathered so far are still a correct,
/// if incomplete, answer) and `Err(reason)` when a hop's source group
/// is not mirrored locally or its history is unreadable; the caller
/// surfaces `reason` as a `warn` note rather than failing the whole
/// call.
async fn follow_history_chain(
    backend: &NativeBackend,
    groups: &GroupIndex,
    mut pointer: CrossGroupHistoryPointer,
    visited: &mut HashSet<(Uuid, String, String)>,
    entries: &mut Vec<OwnedHistoryEntry>,
) -> Result<(), String> {
    loop {
        let key = (
            pointer.source_group,
            pointer.source_path.clone(),
            pointer.last_commit.clone(),
        );
        if !visited.insert(key) {
            return Ok(());
        }

        let Some(source_entry) = groups.get(&GroupId::from_uuid(pointer.source_group)).await else {
            return Err(format!(
                "source group {} is not mirrored locally",
                pointer.source_group
            ));
        };

        let root = Rev::Commit(pointer.last_commit.clone());
        let source_history = walk_path_history_from(
            backend,
            &source_entry.handle,
            &root,
            &pointer.source_path,
            None,
        )
        .await
        .map_err(|err| {
            format!(
                "source group {} history unreadable: {err}",
                pointer.source_group
            )
        })?;
        entries.extend(source_history.into_iter().map(|commit| OwnedHistoryEntry {
            commit,
            owning_group: pointer.source_group,
        }));

        let next_pointer =
            match read_frontmatter_at(backend, &source_entry.handle, &root, &pointer.source_path)
                .await
            {
                Ok(frontmatter) => frontmatter.history_source,
                Err(ImportError::Git(GitError::PathNotFound(_))) => None,
                Err(err) => {
                    return Err(format!(
                        "source group {} pointer target unreadable: {err}",
                        pointer.source_group
                    ));
                }
            };

        match next_pointer {
            Some(next) => pointer = next,
            None => return Ok(()),
        }
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
) -> Result<Option<CrossGroupHistoryPointer>, ImportError> {
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
        let pointer = CrossGroupHistoryPointer::new(
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

    #[tokio::test]
    async fn walk_memory_history_follows_a_chain_of_two_moves_to_reach_the_origin_group() {
        let scratch = ScratchHome::new().await.expect("scratch home");
        let group_a = scratch.seed_group("chain-a").await.expect("seed a");
        let group_b = scratch.seed_group("chain-b").await.expect("seed b");
        let group_c = scratch.seed_group("chain-c").await.expect("seed c");
        let entry_a = scratch
            .groups()
            .get(&group_a.group_id)
            .await
            .expect("entry a");
        let entry_b = scratch
            .groups()
            .get(&group_b.group_id)
            .await
            .expect("entry b");
        let entry_c = scratch
            .groups()
            .get(&group_c.group_id)
            .await
            .expect("entry c");

        let file = MemoryFile {
            frontmatter: MemoryFrontmatter::new("chained", "moved twice", MemoryKind::Scratch),
            body: "origin content\n".to_string(),
            format: FrontmatterFormat::TomlPlus,
        };
        let seeded = import_memory(
            scratch.backend(),
            &entry_a.handle,
            "chained",
            &file.to_string().expect("render"),
            None,
            scratch.author(),
            false,
        )
        .await
        .expect("seed origin memory in a");

        // First hop: A -> B. B's copy now carries a one-hop pointer to A.
        move_memory_across_groups(
            scratch.backend(),
            &entry_a,
            &entry_b,
            None,
            Some(seeded.id),
            scratch.author(),
            CrossGroupMoveOptions::default(),
        )
        .await
        .expect("move a to b");

        // Second hop: B -> C. C's own pointer keeps only ONE hop (to B);
        // B's frozen pre-delete content still carries its own pointer to
        // A, so the walk must recurse through B to reach A.
        move_memory_across_groups(
            scratch.backend(),
            &entry_b,
            &entry_c,
            Some("chained"),
            Some(seeded.id),
            scratch.author(),
            CrossGroupMoveOptions::default(),
        )
        .await
        .expect("move b to c");

        let resolved = resolve_memory(scratch.backend(), &entry_c.handle, Some("chained"), None)
            .await
            .expect("resolve in c");
        let target_frontmatter =
            crate::testing::read_current_frontmatter(scratch.backend(), &entry_c.handle, "chained")
                .await
                .expect("read c frontmatter");
        let pointer = target_frontmatter
            .history_source
            .expect("c carries a pointer");
        assert_eq!(
            pointer.source_group,
            *group_b.group_id.as_uuid(),
            "c's own pointer must name b, one hop, never a directly"
        );

        let outcome = walk_memory_history(
            scratch.backend(),
            &entry_c.handle,
            scratch.groups(),
            &resolved.path,
            None,
        )
        .await
        .expect("walk memory history across the whole chain");

        assert!(outcome.unresolved_pointer.is_none());
        let owning_groups: Vec<Uuid> = outcome.entries.iter().map(|e| e.owning_group).collect();
        assert!(
            owning_groups.contains(group_c.group_id.as_uuid()),
            "must include c's own write commit: {owning_groups:?}"
        );
        assert!(
            owning_groups.contains(group_b.group_id.as_uuid()),
            "must include b's frozen commit: {owning_groups:?}"
        );
        assert!(
            owning_groups.contains(group_a.group_id.as_uuid()),
            "must recurse through b's own pointer to reach a's origin commit: {owning_groups:?}"
        );
    }

    #[tokio::test]
    async fn follow_history_chain_stops_at_a_visited_hop_before_any_lookup() {
        let scratch = ScratchHome::new().await.expect("scratch home");

        // Names a group that was never seeded or mirrored: if the
        // cycle guard did not run BEFORE any group/git lookup, this
        // hop would either error (`groups.get` misses) or, worse,
        // silently attempt a lookup. Pre-seeding the exact hop
        // identity into `visited` must short-circuit before either
        // happens, so this never resolves the group at all.
        let never_mirrored = Uuid::now_v7();
        let pointer = CrossGroupHistoryPointer::new(
            never_mirrored,
            "memories/looped/deadbeef.md",
            "0123456789abcdef0123456789abcdef01234567",
            "fedcba9876543210fedcba9876543210fedcba98",
        )
        .expect("valid pointer shape");

        let key = (
            pointer.source_group,
            pointer.source_path.clone(),
            pointer.last_commit.clone(),
        );
        let mut visited = HashSet::new();
        visited.insert(key);
        let mut entries = Vec::new();

        let outcome = follow_history_chain(
            scratch.backend(),
            scratch.groups(),
            pointer,
            &mut visited,
            &mut entries,
        )
        .await;

        assert!(
            outcome.is_ok(),
            "a hop whose identity was already visited must stop cleanly, never error"
        );
        assert!(
            entries.is_empty(),
            "no walk happens once the hop's key is already visited: {entries:?}"
        );
    }
}
