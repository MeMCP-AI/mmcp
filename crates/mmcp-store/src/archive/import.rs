//! Snapshot import: replay a portable archive back into the local
//! store, recreating groups and writing each memory through the same
//! `import_memory` primitive the loose-file import path uses.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::Read;
use std::path::{Component, Path};

use mmcp_core::conventions::{MEMORIES_DIR, MEMORY_EXTENSION, memory_path};
use mmcp_core::id::{GroupId, MemoryId};
use mmcp_core::manifest::{GroupManifest, GroupScope, MANIFEST_FILENAME};
use mmcp_core::memory::MemoryFile;
use mmcp_git::{CommitSpec, GitBackend, NativeBackend, RepoHandle, Rev};
use uuid::Uuid;

use crate::groups::{GroupEntry, GroupIndex};
use crate::home::ResolvedAuthor;
use crate::lock;
use crate::memory::{
    AddressingMode, ImportError, filename_uuid_from_path, list_memory_slug_dirs, resolve_group,
    validate_id_mismatch, validate_memory_slug, validate_write_content_lengths,
};

use super::error::ArchiveError;
use super::filter::MemoryFilter;
use super::manifest::{
    ARCHIVE_FORMAT_VERSION, ARCHIVE_GIT_DIR, ARCHIVE_GROUPS_DIR, ARCHIVE_MANIFEST_FILENAME,
    ArchiveManifest, ArchiveMode, ArchivedGroupMeta,
};

/// Gzip stream magic; sniffed so import accepts both plain and
/// gzip-compressed archives without the caller declaring which.
const GZIP_MAGIC: [u8; 2] = [0x1f, 0x8b];

/// Hard cap on the total (post-decompression) bytes import reads from an archive,
/// so a gzip bomb or a corrupt length cannot exhaust memory.
/// Generous for archives of text memories.
const MAX_ARCHIVE_BYTES: u64 = 512 * 1024 * 1024;

/// Hard cap on a single archive entry's (post-decompression) bytes.
const MAX_ENTRY_BYTES: u64 = 64 * 1024 * 1024;

/// How an archive should be replayed into the local store.
///
/// `Default`-derived: the common case recreates the original groups
/// by uuid, preserves identities, and skips anything already present.
#[derive(Debug, Clone, Default)]
pub struct ImportArchiveOptions {
    /// Remap every archived memory into this existing local group
    /// instead of recreating the original groups by uuid.
    pub into_group: Option<GroupId>,
    /// Replace a memory whose uuid already exists with differing content.
    /// Off means such a collision is reported, not written.
    pub overwrite: bool,
    /// Mint fresh UUIDs for every imported memory (fork / copy)
    /// rather than preserving the archived identities.
    pub new_ids: bool,
    /// Permit writes into protected existing groups.
    /// The surfaces set this only after confirming the write with the operator.
    pub allow_protected: bool,
    /// When non-empty, import only these archived groups (matched by uuid or slug).
    /// Empty imports every group in the archive.
    pub select_groups: Vec<String>,
    /// Facet filter narrowing which memories are replayed.
    /// Empty imports every memory in the selected groups.
    /// Snapshot mode only.
    pub filter: MemoryFilter,
    /// History mode: overwrite a group that already exists locally with the restored repo.
    /// Off skips groups already present (the clean machine case installs them either way).
    pub force_restore: bool,
}

/// A memory the import left untouched because its uuid already exists
/// with differing content and `overwrite` was not set.
#[derive(Debug, Clone)]
pub struct MemoryConflict {
    pub slug: String,
    pub id: Uuid,
}

/// Per-group outcome of an import run.
#[derive(Debug, Clone)]
pub struct GroupImportOutcome {
    /// Group uuid recorded in the archive.
    pub source_group_id: Uuid,
    /// Group uuid the memories actually landed in (differs from
    /// `source_group_id` only under `into_group` remapping).
    pub target_group_id: Uuid,
    /// Group slug recorded in the archive.
    pub slug: String,
    /// True when the target group repo was created by this import.
    pub created_group: bool,
    /// Memories newly written.
    pub created: u32,
    /// Memories replaced in place under `overwrite`.
    pub overwritten: u32,
    /// Memories skipped because an identical copy already existed.
    pub skipped: u32,
    /// Memories left untouched on a differing-content collision.
    pub conflicts: Vec<MemoryConflict>,
}

/// Summary of an import run across every archived group.
#[derive(Debug, Clone, Default)]
pub struct ImportArchiveReport {
    pub groups: Vec<GroupImportOutcome>,
}

/// Read just the table of contents from an archive without importing.
///
/// Surfaces use this to list contents, drive a dry run, and discover
/// which target groups a protected-group confirmation must cover.
pub fn inspect_archive(bytes: &[u8]) -> Result<ArchiveManifest, ArchiveError> {
    let reader = open_reader(bytes);
    let mut archive = tar::Archive::new(reader);
    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.to_string_lossy().into_owned();
        if path == ARCHIVE_MANIFEST_FILENAME {
            let buf = read_capped(&mut entry, &path)?;
            let text = utf8(&path, &buf)?;
            let manifest = ArchiveManifest::from_toml(text)?;
            ensure_supported(&manifest)?;
            return Ok(manifest);
        }
    }
    Err(ArchiveError::MissingManifest)
}

/// One archived group's contents: its identity, scope, the memory
/// slugs it carries, and the distinct tags across those memories.
/// Surfaces use this to drive an import selection UI with scope
/// grouping and tag autocomplete.
#[derive(Debug, Clone)]
pub struct ArchiveGroupListing {
    pub group_id: Uuid,
    pub slug: String,
    pub scope: GroupScope,
    pub memory_slugs: Vec<String>,
    pub tags: Vec<String>,
}

/// List every group in an archive with its scope, memory slugs, and
/// distinct tags, so a selection UI can offer scope grouping plus
/// group-, memory-, and tag-level choices before an import runs.
pub fn list_archive(bytes: &[u8]) -> Result<Vec<ArchiveGroupListing>, ArchiveError> {
    let entries = read_entries(bytes)?;
    let manifest = read_toc(&entries)?;
    ensure_supported(&manifest)?;

    let mut listings = Vec::with_capacity(manifest.groups.len());
    for group_meta in &manifest.groups {
        let group_id = group_meta.group_id;
        let scope = entries
            .get(&format!(
                "{ARCHIVE_GROUPS_DIR}/{group_id}/{MANIFEST_FILENAME}"
            ))
            .and_then(|bytes| std::str::from_utf8(bytes).ok())
            .and_then(|text| GroupManifest::from_toml(text).ok())
            .map(|manifest| manifest.scope)
            .unwrap_or_default();

        let prefix = format!("{ARCHIVE_GROUPS_DIR}/{group_id}/{MEMORIES_DIR}/");
        let mut memory_slugs: Vec<String> = Vec::new();
        let mut tags: Vec<String> = Vec::new();
        for (path, data) in &entries {
            if !path.ends_with(MEMORY_EXTENSION) {
                continue;
            }
            let Some(remainder) = path.strip_prefix(&prefix) else {
                continue;
            };
            let Some((slug, _)) = remainder.rsplit_once('/') else {
                continue;
            };
            if !memory_slugs.iter().any(|s| s == slug) {
                memory_slugs.push(slug.to_string());
            }
            if let Ok(text) = std::str::from_utf8(data)
                && let Ok(parsed) = MemoryFile::parse(text)
            {
                for tag in parsed.frontmatter.tags {
                    if !tags.iter().any(|t| t == &tag) {
                        tags.push(tag);
                    }
                }
            }
        }
        memory_slugs.sort();
        tags.sort();
        listings.push(ArchiveGroupListing {
            group_id,
            slug: group_meta.slug.clone(),
            scope,
            memory_slugs,
            tags,
        });
    }
    Ok(listings)
}

/// Replay `bytes` into the local store and report what happened.
/// Branches on the archive mode: a snapshot replays memories, a history
/// archive restores each group's bare repo verbatim.
pub async fn import_archive(
    backend: &NativeBackend,
    groups: &GroupIndex,
    author: &ResolvedAuthor,
    bytes: &[u8],
    options: &ImportArchiveOptions,
) -> Result<ImportArchiveReport, ArchiveError> {
    let entries = read_entries(bytes)?;
    let manifest = read_toc(&entries)?;
    ensure_supported(&manifest)?;

    match manifest.mode {
        ArchiveMode::Snapshot => {
            import_snapshot(backend, groups, author, &entries, &manifest, options).await
        }
        ArchiveMode::History => import_history(backend, groups, &entries, &manifest, options).await,
    }
}

/// Snapshot replay: recreate or merge each group and write every memory
/// through the shared `import_memory` primitive.
async fn import_snapshot(
    backend: &NativeBackend,
    groups: &GroupIndex,
    author: &ResolvedAuthor,
    entries: &BTreeMap<String, Vec<u8>>,
    manifest: &ArchiveManifest,
    options: &ImportArchiveOptions,
) -> Result<ImportArchiveReport, ArchiveError> {
    // Resolve the single remap target up front when --into is set.
    let into_target = match options.into_group {
        Some(group_id) => Some(
            groups
                .get(&group_id)
                .await
                .ok_or_else(|| ArchiveError::IntoGroupNotFound(group_id.as_uuid().to_string()))?,
        ),
        None => None,
    };

    // Backstop the protected-group guard before any write so a partial
    // import cannot start against a group the caller has not confirmed.
    protected_precheck(groups, manifest, into_target.as_ref(), options).await?;

    let mut report = ImportArchiveReport::default();
    for group_meta in &manifest.groups {
        if !group_selected(&options.select_groups, group_meta) {
            continue;
        }
        let outcome = import_one_group(
            backend,
            groups,
            author,
            entries,
            group_meta.group_id,
            group_meta.slug.clone(),
            into_target.as_ref(),
            options,
        )
        .await?;
        report.groups.push(outcome);
    }
    Ok(report)
}

/// History restore: install each selected group's bare repo verbatim, preserving full git history.
/// Whole-repo and all-or-nothing per group, so the snapshot-only knobs are rejected up front.
async fn import_history(
    backend: &NativeBackend,
    groups: &GroupIndex,
    entries: &BTreeMap<String, Vec<u8>>,
    manifest: &ArchiveManifest,
    options: &ImportArchiveOptions,
) -> Result<ImportArchiveReport, ArchiveError> {
    if options.into_group.is_some() {
        return Err(ArchiveError::SnapshotOnlyOption {
            option: "into_group",
        });
    }
    if options.new_ids {
        return Err(ArchiveError::SnapshotOnlyOption { option: "new_ids" });
    }
    if options.overwrite {
        return Err(ArchiveError::SnapshotOnlyOption {
            option: "overwrite",
        });
    }
    if !options.filter.is_empty() {
        return Err(ArchiveError::SnapshotOnlyOption { option: "filter" });
    }

    // A forced restore can overwrite an existing (possibly protected) group,
    // so it takes the same confirmation backstop.
    // Without force, existing groups are skipped and nothing is overwritten.
    if options.force_restore {
        protected_precheck(groups, manifest, None, options).await?;
    }

    let mut report = ImportArchiveReport::default();
    for group_meta in &manifest.groups {
        if !group_selected(&options.select_groups, group_meta) {
            continue;
        }
        let outcome =
            restore_one_group(backend, groups, entries, group_meta, options.force_restore).await?;
        report.groups.push(outcome);
    }
    Ok(report)
}

/// Install one archived group's bare repo, skipping it when it already exists unless `force_restore`.
/// On a clean machine the group is absent and is installed fresh.
async fn restore_one_group(
    backend: &NativeBackend,
    groups: &GroupIndex,
    entries: &BTreeMap<String, Vec<u8>>,
    group_meta: &ArchivedGroupMeta,
    force_restore: bool,
) -> Result<GroupImportOutcome, ArchiveError> {
    let group_id = group_meta.group_id;
    let exists = resolve_group(groups, &group_id.to_string()).await.is_ok();

    let mut outcome = GroupImportOutcome {
        source_group_id: group_id,
        target_group_id: group_id,
        slug: group_meta.slug.clone(),
        created_group: false,
        created: 0,
        overwritten: 0,
        skipped: 0,
        conflicts: Vec::new(),
    };

    if exists && !force_restore {
        outcome.skipped = group_meta.memory_count;
        return Ok(outcome);
    }

    let git_prefix = format!("{ARCHIVE_GROUPS_DIR}/{group_id}/{ARCHIVE_GIT_DIR}/");
    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    for (path, data) in entries {
        let Some(rel) = path.strip_prefix(&git_prefix) else {
            continue;
        };
        if !is_safe_relpath(rel) {
            return Err(ArchiveError::Malformed {
                detail: format!("history archive has an unsafe git path `{path}`"),
            });
        }
        files.push((rel.to_string(), data.clone()));
    }
    if files.is_empty() {
        return Err(ArchiveError::Malformed {
            detail: format!("history archive carries no git files for group {group_id}"),
        });
    }

    let repo_path = backend.repo_path(group_id);
    let backend_for_install = backend.clone();
    tokio::task::spawn_blocking(move || {
        install_bare_repo(&backend_for_install, &repo_path, &files)
    })
    .await
    .map_err(|e| ArchiveError::Malformed {
        detail: format!("restore task failed: {e}"),
    })??;

    groups.refresh().await?;

    // The scanner drops a repo whose manifest id disagrees with its
    // directory name; surface that instead of a silently-missing group.
    resolve_group(groups, &group_id.to_string())
        .await
        .map_err(|_| ArchiveError::Malformed {
            detail: format!(
                "restored group {group_id} did not register; its manifest id likely mismatches"
            ),
        })?;

    if exists {
        outcome.overwritten = group_meta.memory_count;
    } else {
        outcome.created_group = true;
        outcome.created = group_meta.memory_count;
    }
    Ok(outcome)
}

/// Atomically install a bare repo's `files` at `repo_path`: stage in a sibling temp dir,
/// then rename into place so a partial write never leaves a broken repo.
/// Replaces an existing repo (the caller gates that on `force_restore`).
///
/// Invalidates `backend`'s cached repo handle for `repo_path` immediately
/// after the swap completes: the rename-aside/rename-in sequence below
/// leaves the path existing throughout, so `NativeBackend::open_repo`'s
/// `path.exists()` validity check alone cannot detect the in-place
/// replacement and would keep serving the pre-restore content.
fn install_bare_repo(
    backend: &NativeBackend,
    repo_path: &Path,
    files: &[(String, Vec<u8>)],
) -> Result<(), ArchiveError> {
    let parent = repo_path.parent().ok_or_else(|| ArchiveError::Malformed {
        detail: format!("repo path {} has no parent", repo_path.display()),
    })?;
    let pid = std::process::id();
    let stage = parent.join(format!(".restore-{pid}.stage"));
    let _ = std::fs::remove_dir_all(&stage);
    for (rel, bytes) in files {
        let dest = stage.join(rel);
        if let Some(dir) = dest.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(&dest, bytes)?;
    }

    if repo_path.exists() {
        let backup = parent.join(format!(".restore-{pid}.old"));
        let _ = std::fs::remove_dir_all(&backup);
        std::fs::rename(repo_path, &backup)?;
        if let Err(e) = std::fs::rename(&stage, repo_path) {
            let _ = std::fs::rename(&backup, repo_path);
            return Err(e.into());
        }
        let _ = std::fs::remove_dir_all(&backup);
    } else {
        std::fs::rename(&stage, repo_path)?;
    }
    backend.invalidate(repo_path);
    Ok(())
}

/// Whether `rel` is a safe relative path to extract:
/// every component must be a plain name (no `..`, no root, no drive prefix),
/// so a crafted archive cannot escape the staging directory.
fn is_safe_relpath(rel: &str) -> bool {
    !rel.is_empty()
        && Path::new(rel)
            .components()
            .all(|c| matches!(c, Component::Normal(_)))
}

/// Import every memory belonging to one archived group, recreating or
/// merging the target group as needed.
///
/// Snapshots the target group's existing memories ONCE (via
/// [`snapshot_existing_memories`]) instead of running a `resolve_memory(.., None, Some(id))`
/// full-corpus walk per imported memory, and accumulates every create/overwrite into ONE
/// [`CommitSpec::mmcp_commit`] instead of one commit per memory: an M-memory import into an
/// N-memory group used to cost M full-corpus walks plus M commits; this costs one walk
/// (plus one batched read of every existing file) and one commit for the whole group.
/// An all-skip/all-conflict group produces no commit at all.
#[allow(clippy::too_many_arguments)]
async fn import_one_group(
    backend: &NativeBackend,
    groups: &GroupIndex,
    author: &ResolvedAuthor,
    entries: &BTreeMap<String, Vec<u8>>,
    source_group_id: Uuid,
    slug: String,
    into_target: Option<&GroupEntry>,
    options: &ImportArchiveOptions,
) -> Result<GroupImportOutcome, ArchiveError> {
    let (target, created_group) = match into_target {
        Some(target) => (target.clone(), false),
        None => resolve_or_create_target(backend, groups, entries, source_group_id).await?,
    };

    let mut outcome = GroupImportOutcome {
        source_group_id,
        target_group_id: target.handle.group_id,
        slug,
        created_group,
        created: 0,
        overwritten: 0,
        skipped: 0,
        conflicts: Vec::new(),
    };

    // One exclusive view of the whole group for the entire batch below, in place of
    // `import_memory`/`write_file_at_path`'s own per-memory `create_chain` lock: every
    // accumulated memory commits together in the ONE `write_commit` at the end of this
    // function, so a lock held for the whole batch already covers what a per-memory
    // lock/unlock cycle would, without the added contention.
    let _guards = lock::acquire_chain(&lock::create_chain(target.handle.group_id)).await;

    let mut existing = snapshot_existing_memories(backend, &target.handle).await?;
    let mut commit_files: Vec<(String, Option<Vec<u8>>)> = Vec::new();
    let mut pending: Vec<PendingCacheNotify> = Vec::new();

    let prefix = format!("{ARCHIVE_GROUPS_DIR}/{source_group_id}/{MEMORIES_DIR}/");
    for (path, data) in entries {
        if !path.starts_with(&prefix) || !path.ends_with(MEMORY_EXTENSION) {
            continue;
        }
        let remainder = &path[prefix.len()..];
        let Some((memory_slug, filename)) = remainder.rsplit_once('/') else {
            return Err(ArchiveError::Malformed {
                detail: format!("memory entry `{path}` has no slug directory"),
            });
        };
        let content = utf8(path, data)?;
        if !options.filter.is_empty() {
            let parsed = MemoryFile::parse(content).map_err(ImportError::Parse)?;
            if !options
                .filter
                .matches(memory_slug, &parsed.frontmatter, &parsed.body)
            {
                continue;
            }
        }
        // The archive filename is `<uuid>.md`; the uuid is the memory's
        // identity for id-less frontmatter (hand-crafted memories).
        let filename_id = filename
            .strip_suffix(MEMORY_EXTENSION)
            .and_then(|stem| Uuid::parse_str(stem).ok());
        accumulate_one_memory(
            memory_slug,
            filename_id,
            content,
            options,
            &mut existing,
            &mut commit_files,
            &mut pending,
            &mut outcome,
        )?;
    }

    if commit_files.is_empty() {
        return Ok(outcome);
    }

    let memory_word = if commit_files.len() == 1 {
        "memory"
    } else {
        "memories"
    };
    let commit_message = format!(
        "import {} {memory_word} into {}",
        commit_files.len(),
        target.manifest.slug
    );
    let commit_id = backend
        .write_commit(
            &target.handle,
            CommitSpec::mmcp_commit(commit_message, commit_files, &author.name, &author.email),
        )
        .await?;

    // Write-trigger for the local content cache (see `crate::cache`), mirroring
    // `write_file_at_path`'s per-write hook: best-effort, one notification per
    // accumulated memory, all sharing the single commit id the batch produced above.
    for note in pending {
        crate::cache::notify_write(
            target.handle.group_id,
            note.id,
            &note.slug,
            &note.path,
            &commit_id,
            &note.rendered,
        )
        .await;
    }

    Ok(outcome)
}

/// Resolve the target group by the archived uuid, creating it from the
/// archived `.mmcp.toml` when it is not yet in the local mirror.
async fn resolve_or_create_target(
    backend: &NativeBackend,
    groups: &GroupIndex,
    entries: &BTreeMap<String, Vec<u8>>,
    source_group_id: Uuid,
) -> Result<(GroupEntry, bool), ArchiveError> {
    match resolve_group(groups, &source_group_id.to_string()).await {
        Ok(existing) => Ok((existing, false)),
        Err(ImportError::GroupNotFound(_)) => {
            let manifest_path =
                format!("{ARCHIVE_GROUPS_DIR}/{source_group_id}/{MANIFEST_FILENAME}");
            let bytes = entries
                .get(&manifest_path)
                .ok_or(ArchiveError::GroupManifestMissing {
                    group_id: source_group_id,
                })?;
            let text = utf8(&manifest_path, bytes)?;
            let manifest = GroupManifest::from_toml(text).map_err(|source| {
                ArchiveError::GroupManifestParse {
                    group_id: source_group_id,
                    source,
                }
            })?;
            // The repo is created at manifest.group_id; reject an archive
            // whose inner manifest disagrees with its directory uuid so a
            // crafted archive cannot persist a stray repo under a
            // different identity than the operator confirmed against.
            if manifest.group_id.as_uuid() != &source_group_id {
                return Err(ArchiveError::Malformed {
                    detail: format!(
                        "group {source_group_id} manifest declares a different id {}",
                        manifest.group_id.as_uuid()
                    ),
                });
            }
            backend.create_group_repo(&manifest).await?;
            groups.refresh().await?;
            let entry = groups
                .get(&GroupId::from_uuid(source_group_id))
                .await
                .ok_or(ArchiveError::GroupManifestMissing {
                    group_id: source_group_id,
                })?;
            Ok((entry, true))
        }
        Err(other) => Err(other.into()),
    }
}

/// One pre-existing memory in a target group, snapshotted once per
/// [`import_one_group`] call.
///
/// Keyed by its FRONTMATTER id, the same source of truth
/// [`crate::memory::resolve_memory`]'s `resolve_by_id` fallback scans for,
/// not its filename stem: a hand-crafted or filename-drifted file is found
/// exactly like a per-memory `resolve_memory(.., None, Some(id))` call would find it.
struct ExistingMemory {
    slug: String,
    path: String,
    text: String,
    addressing_mode: AddressingMode,
}

/// Snapshot of every memory currently in a target group, built once per
/// [`import_one_group`] call instead of once per imported memory.
struct ExistingSnapshot {
    by_frontmatter_id: HashMap<Uuid, ExistingMemory>,
    occupied_paths: HashSet<String>,
}

impl ExistingSnapshot {
    /// Record a memory this same import run just staged for creation, so a LATER
    /// duplicate id or canonical-path collision within the same archive is caught
    /// against it exactly like a pre-existing one, without a second git round trip.
    fn record(&mut self, id: Uuid, entry: ExistingMemory) {
        self.occupied_paths.insert(entry.path.clone());
        self.by_frontmatter_id.insert(id, entry);
    }
}

/// Snapshot every memory in `handle` in one walk plus one batched read.
///
/// Replaces the per-imported-memory `resolve_memory(.., None, Some(id))` call: that walked
/// the whole group's memory files sequentially for every imported memory, so an M-memory
/// import into an N-memory group cost M full-corpus walks. This walks the group exactly
/// once (mirroring `resolve_by_id`'s own candidate enumeration, not `list_all_memory_files`,
/// so a hand-crafted non-UUID-named file is snapshotted too) and folds every later id lookup
/// through the resulting map instead of a git round trip.
async fn snapshot_existing_memories(
    backend: &NativeBackend,
    handle: &RepoHandle,
) -> Result<ExistingSnapshot, ArchiveError> {
    let rev = Rev::head();
    let slug_dirs = list_memory_slug_dirs(backend, handle, &rev).await?;
    let candidates: Vec<(String, String)> = slug_dirs
        .iter()
        .flat_map(|dir| {
            dir.filenames
                .iter()
                .filter(|filename| filename.ends_with(MEMORY_EXTENSION))
                .map(move |filename| (dir.slug.clone(), format!("{}/{filename}", dir.dir)))
        })
        .collect();

    let occupied_paths: HashSet<String> = candidates.iter().map(|(_, path)| path.clone()).collect();
    let mut by_frontmatter_id = HashMap::new();
    if !candidates.is_empty() {
        let paths: Vec<String> = candidates.iter().map(|(_, path)| path.clone()).collect();
        // One resolve of the commit and root tree, reused for every path below,
        // instead of the former per-memory resolve_by_id's repeated walks.
        let batch = backend.read_files(handle, paths, &rev).await?;
        for ((slug, path), (_batch_path, outcome)) in candidates.iter().zip(batch) {
            let Ok(bytes) = outcome else { continue };
            let Ok(text) = std::str::from_utf8(&bytes) else {
                continue;
            };
            let Ok(parsed) = MemoryFile::parse(text) else {
                continue;
            };
            let Some(id) = parsed.frontmatter.id else {
                continue;
            };
            let addressing_mode = if filename_uuid_from_path(path) == Some(id) {
                AddressingMode::ByFilename
            } else {
                AddressingMode::ByFrontmatter
            };
            by_frontmatter_id.insert(
                id,
                ExistingMemory {
                    slug: slug.clone(),
                    path: path.clone(),
                    text: text.to_string(),
                    addressing_mode,
                },
            );
        }
    }
    Ok(ExistingSnapshot {
        by_frontmatter_id,
        occupied_paths,
    })
}

/// One accumulated write pending the group's single batched commit,
/// staged so [`import_one_group`] can fire the cache write-trigger
/// hook once the real commit id exists.
struct PendingCacheNotify {
    id: Uuid,
    slug: String,
    path: String,
    rendered: String,
}

/// Re-parse `content`, stamp `id` into its frontmatter, and render: the exact pipeline
/// `import_memory` runs on create, so the batched path commits byte-identical content
/// to the per-memory primitive it replaces.
fn mint_and_render(content: &str, id: Uuid) -> Result<String, ArchiveError> {
    let mut parsed = MemoryFile::parse(content).map_err(ImportError::Parse)?;
    parsed.frontmatter = parsed.frontmatter.clone().with_id(id);
    let rendered = parsed
        .to_string()
        .map_err(|e| ImportError::Render(e.to_string()))?;
    Ok(rendered)
}

/// Stage a brand-new memory at its canonical `memories/<slug>/<id>.md` path, rejecting a
/// path collision exactly like [`crate::memory::write_memory_by_id`]'s exists-probe does
/// with `override_existing = false`.
#[allow(clippy::too_many_arguments)]
fn stage_create(
    slug: &str,
    id: Uuid,
    rendered: String,
    existing: &mut ExistingSnapshot,
    commit_files: &mut Vec<(String, Option<Vec<u8>>)>,
    pending: &mut Vec<PendingCacheNotify>,
    outcome: &mut GroupImportOutcome,
) -> Result<(), ArchiveError> {
    validate_memory_slug(slug)?;
    // Archive restore never refuses existing content.
    // Only the hard body-length bound applies here.
    // The write-time inline-result ceiling is reserved for a caller-authored write.
    validate_write_content_lengths(&rendered)?;
    let path = memory_path(slug, MemoryId::from_uuid(id));
    if existing.occupied_paths.contains(&path) {
        return Err(ImportError::MemoryAlreadyExists {
            slug: slug.to_string(),
        }
        .into());
    }
    commit_files.push((path.clone(), Some(rendered.as_bytes().to_vec())));
    existing.record(
        id,
        ExistingMemory {
            slug: slug.to_string(),
            path: path.clone(),
            text: rendered.clone(),
            addressing_mode: AddressingMode::ByFilename,
        },
    );
    pending.push(PendingCacheNotify {
        id,
        slug: slug.to_string(),
        path,
        rendered,
    });
    outcome.created += 1;
    Ok(())
}

/// Accumulate one archived memory's effect into the batch's pending commit state,
/// applying the new-ids / overwrite / skip policy and tallying the result on `outcome`.
///
/// Does no I/O: every existing-content read the former per-memory
/// `resolve_memory` + `read_file` pair performed was already folded into `existing`
/// by [`snapshot_existing_memories`] before the caller's loop began.
#[allow(clippy::too_many_arguments)]
fn accumulate_one_memory(
    slug: &str,
    filename_id: Option<Uuid>,
    content: &str,
    options: &ImportArchiveOptions,
    existing: &mut ExistingSnapshot,
    commit_files: &mut Vec<(String, Option<Vec<u8>>)>,
    pending: &mut Vec<PendingCacheNotify>,
    outcome: &mut GroupImportOutcome,
) -> Result<(), ArchiveError> {
    // Fork semantics: drop the archived id so a fresh one is minted and
    // the memory always lands as a new sibling.
    if options.new_ids {
        let forked = content_without_id(content)?;
        let id = Uuid::now_v7();
        let rendered = mint_and_render(&forked, id)?;
        return stage_create(slug, id, rendered, existing, commit_files, pending, outcome);
    }

    // Identity-preserving import.
    // The id is the frontmatter id, falling back to the archive filename uuid
    // so id-less files keep their identity and a re-import stays idempotent
    // rather than minting a fresh duplicate every run.
    let Some((id, prepared)) = ensure_id(content, filename_id)? else {
        return Err(ArchiveError::Malformed {
            detail: format!("memory `{slug}` has no id in frontmatter or filename"),
        });
    };

    match existing.by_frontmatter_id.get(&id) {
        None => {
            let rendered = mint_and_render(&prepared, id)?;
            stage_create(slug, id, rendered, existing, commit_files, pending, outcome)
        }
        Some(found) => {
            if normalize(&found.text)? == normalize(&prepared)? {
                outcome.skipped += 1;
                return Ok(());
            }
            if !options.overwrite {
                outcome.conflicts.push(MemoryConflict {
                    slug: found.slug.clone(),
                    id,
                });
                return Ok(());
            }
            // Replace at the existing on-disk path so a drifted memory (filename
            // uuid != frontmatter id) is replaced in place rather than duplicated
            // under a canonical name. `force = true` mirrors the original overwrite
            // branch's `WriteFileOptions { force: true, .. }`; the mismatch check
            // can only return an accepted/forced outcome, never an error, once
            // `force` is set, so this cannot fail on a genuine drift.
            let found_path = found.path.clone();
            let found_slug = found.slug.clone();
            let found_mode = found.addressing_mode;
            validate_id_mismatch(&found_path, &prepared, found_mode, true)?;
            validate_write_content_lengths(&prepared)?;
            commit_files.push((found_path.clone(), Some(prepared.as_bytes().to_vec())));
            existing.by_frontmatter_id.insert(
                id,
                ExistingMemory {
                    slug: found_slug.clone(),
                    path: found_path.clone(),
                    text: prepared.clone(),
                    addressing_mode: found_mode,
                },
            );
            pending.push(PendingCacheNotify {
                id,
                slug: found_slug,
                path: found_path,
                rendered: prepared,
            });
            outcome.overwritten += 1;
            Ok(())
        }
    }
}

/// Error before any write when an archive targets a protected existing
/// group the caller has not opted into writing.
async fn protected_precheck(
    groups: &GroupIndex,
    manifest: &ArchiveManifest,
    into_target: Option<&GroupEntry>,
    options: &ImportArchiveOptions,
) -> Result<(), ArchiveError> {
    if options.allow_protected {
        return Ok(());
    }
    if let Some(target) = into_target {
        if target.manifest.protected {
            return Err(ArchiveError::ProtectedGroup {
                group_id: target.handle.group_id,
                slug: target.manifest.slug.clone(),
            });
        }
        return Ok(());
    }
    for group_meta in &manifest.groups {
        if !group_selected(&options.select_groups, group_meta) {
            continue;
        }
        if let Ok(existing) = resolve_group(groups, &group_meta.group_id.to_string()).await
            && existing.manifest.protected
        {
            return Err(ArchiveError::ProtectedGroup {
                group_id: existing.handle.group_id,
                slug: existing.manifest.slug.clone(),
            });
        }
    }
    Ok(())
}

/// Whether an archived group is in scope for this import.
/// An empty selection imports every group; otherwise a group matches by its uuid string or its slug.
fn group_selected(select: &[String], group_meta: &ArchivedGroupMeta) -> bool {
    select.is_empty()
        || select
            .iter()
            .any(|s| s == &group_meta.group_id.to_string() || s == &group_meta.slug)
}

/// Reject an archive whose layout version is newer than this build.
fn ensure_supported(manifest: &ArchiveManifest) -> Result<(), ArchiveError> {
    if manifest.format_version > ARCHIVE_FORMAT_VERSION {
        return Err(ArchiveError::UnsupportedFormatVersion {
            found: manifest.format_version,
            supported: ARCHIVE_FORMAT_VERSION,
        });
    }
    Ok(())
}

/// Read every tar entry into a path-keyed map, transparently decompressing a gzip stream.
/// Rejects duplicate paths so a crafted archive cannot shadow the table of contents
/// the protected-group confirmation was driven from.
fn read_entries(bytes: &[u8]) -> Result<BTreeMap<String, Vec<u8>>, ArchiveError> {
    let reader = open_reader(bytes);
    let mut archive = tar::Archive::new(reader);
    let mut map = BTreeMap::new();
    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.to_string_lossy().into_owned();
        let buf = read_capped(&mut entry, &path)?;
        if map.insert(path.clone(), buf).is_some() {
            return Err(ArchiveError::Malformed {
                detail: format!("duplicate archive entry `{path}`"),
            });
        }
    }
    Ok(map)
}

/// Read one entry's bytes, rejecting anything past the per-entry cap.
fn read_capped<R: Read>(entry: &mut R, path: &str) -> Result<Vec<u8>, ArchiveError> {
    let mut buf = Vec::new();
    entry.take(MAX_ENTRY_BYTES + 1).read_to_end(&mut buf)?;
    if buf.len() as u64 > MAX_ENTRY_BYTES {
        return Err(ArchiveError::Malformed {
            detail: format!("archive entry `{path}` exceeds the per-entry size limit"),
        });
    }
    Ok(buf)
}

/// Look up and parse the `archive.toml` table of contents.
fn read_toc(entries: &BTreeMap<String, Vec<u8>>) -> Result<ArchiveManifest, ArchiveError> {
    let bytes = entries
        .get(ARCHIVE_MANIFEST_FILENAME)
        .ok_or(ArchiveError::MissingManifest)?;
    let text = utf8(ARCHIVE_MANIFEST_FILENAME, bytes)?;
    Ok(ArchiveManifest::from_toml(text)?)
}

/// Wrap the raw bytes in a (multi-member) gzip decoder when the gzip
/// magic is present, then cap the total decompressed bytes so a bomb
/// cannot exhaust memory.
fn open_reader(bytes: &[u8]) -> Box<dyn Read + '_> {
    let raw: Box<dyn Read + '_> =
        if bytes.len() >= GZIP_MAGIC.len() && bytes[..GZIP_MAGIC.len()] == GZIP_MAGIC {
            Box::new(flate2::read::MultiGzDecoder::new(bytes))
        } else {
            Box::new(bytes)
        };
    Box::new(raw.take(MAX_ARCHIVE_BYTES))
}

/// Decode an archive entry's bytes as UTF-8, attributing failures to
/// the entry path.
fn utf8<'a>(path: &str, bytes: &'a [u8]) -> Result<&'a str, ArchiveError> {
    std::str::from_utf8(bytes).map_err(|source| ArchiveError::NotUtf8 {
        path: path.to_string(),
        source,
    })
}

/// Re-render a memory document with its frontmatter id removed.
fn content_without_id(content: &str) -> Result<String, ArchiveError> {
    let mut parsed = MemoryFile::parse(content).map_err(ImportError::Parse)?;
    parsed.frontmatter.id = None;
    let rendered = parsed.to_string().map_err(ImportError::Parse)?;
    Ok(rendered)
}

/// Resolve a memory's effective id (frontmatter id, else the archive filename uuid),
/// and return the content guaranteed to carry it.
/// Yields `None` only when the memory has no id in either place:
/// an archive whose identity cannot be preserved.
fn ensure_id(
    content: &str,
    fallback: Option<Uuid>,
) -> Result<Option<(Uuid, String)>, ArchiveError> {
    let mut parsed = MemoryFile::parse(content).map_err(ImportError::Parse)?;
    if let Some(id) = parsed.frontmatter.id {
        return Ok(Some((id, content.to_string())));
    }
    let Some(id) = fallback else {
        return Ok(None);
    };
    parsed.frontmatter.id = Some(id);
    let rendered = parsed.to_string().map_err(ImportError::Parse)?;
    Ok(Some((id, rendered)))
}

/// Re-render a memory document in canonical form so two copies compare
/// equal regardless of incidental formatting differences.
fn normalize(content: &str) -> Result<String, ArchiveError> {
    let parsed = MemoryFile::parse(content).map_err(ImportError::Parse)?;
    let rendered = parsed.to_string().map_err(ImportError::Parse)?;
    Ok(rendered)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::super::manifest::{ArchiveMode, ArchivedGroupMeta};
    use super::*;
    use crate::memory::{import_memory, resolve_memory};
    use crate::testing::ScratchHome;
    use crate::{ExportOptions, export_archive, list_all_memory_files};

    /// Build a minimal memory document with an explicit id so tests
    /// control the round-trip identity.
    fn memory_doc(id: Uuid, body: &str) -> String {
        format!(
            "+++\nid = \"{id}\"\nname = \"note\"\ndescription = \"d\"\nkind = \"reference\"\n+++\n{body}\n"
        )
    }

    async fn export_group(home: &ScratchHome, group: GroupId) -> Vec<u8> {
        let entry = home.groups().get(&group).await.expect("group entry");
        let mut buf = Vec::new();
        export_archive(
            home.backend(),
            &[entry],
            &ExportOptions::default(),
            &mut buf,
        )
        .await
        .expect("export");
        buf
    }

    /// Hand-build an archive carrying one memory whose frontmatter body is supplied verbatim,
    /// used to construct id-less inputs the store's own export path never produces.
    fn build_archive(
        group_id: Uuid,
        group_manifest: &GroupManifest,
        slug: &str,
        memory_id: Uuid,
        memory_body: &str,
    ) -> Vec<u8> {
        let toc = ArchiveManifest {
            format_version: ARCHIVE_FORMAT_VERSION,
            mode: ArchiveMode::Snapshot,
            mmcp_version: "test".to_string(),
            created_at: "2026-01-01T00:00:00Z".to_string(),
            groups: vec![ArchivedGroupMeta {
                group_id,
                slug: group_manifest.slug.clone(),
                display_name: None,
                memory_count: 1,
            }],
        }
        .to_toml()
        .expect("toc");
        let group_toml = group_manifest.to_toml().expect("group manifest");

        let mut buf = Vec::new();
        {
            let mut builder = tar::Builder::new(&mut buf);
            append_entry(&mut builder, ARCHIVE_MANIFEST_FILENAME, toc.as_bytes());
            append_entry(
                &mut builder,
                &format!("{ARCHIVE_GROUPS_DIR}/{group_id}/{MANIFEST_FILENAME}"),
                group_toml.as_bytes(),
            );
            append_entry(
                &mut builder,
                &format!("{ARCHIVE_GROUPS_DIR}/{group_id}/{MEMORIES_DIR}/{slug}/{memory_id}.md"),
                memory_body.as_bytes(),
            );
            builder.finish().expect("finish tar");
        }
        buf
    }

    fn append_entry(builder: &mut tar::Builder<&mut Vec<u8>>, path: &str, data: &[u8]) {
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::Regular);
        header.set_size(data.len() as u64);
        header.set_mode(0o644);
        builder
            .append_data(&mut header, path, data)
            .expect("append");
    }

    #[test]
    fn ensure_id_falls_back_to_filename_then_errors_when_absent() {
        let filename_id = Uuid::now_v7();
        let no_id = "+++\nname = \"n\"\ndescription = \"d\"\nkind = \"reference\"\n+++\nBody.\n";
        let (effective, prepared) = ensure_id(no_id, Some(filename_id))
            .expect("ensure")
            .expect("has id");
        assert_eq!(effective, filename_id);
        assert!(
            prepared.contains(&filename_id.to_string()),
            "filename id must be injected into frontmatter",
        );

        // An explicit frontmatter id wins and the content is unchanged.
        let explicit = Uuid::now_v7();
        let doc = memory_doc(explicit, "Body.");
        let (effective2, prepared2) = ensure_id(&doc, Some(Uuid::now_v7()))
            .expect("ensure")
            .expect("has id");
        assert_eq!(effective2, explicit);
        assert_eq!(prepared2, doc);

        // No id anywhere is unresolvable.
        assert!(ensure_id(no_id, None).expect("ensure").is_none());
    }

    /// History export of a group, restored into a clean home, brings the whole bare repo back:
    /// the memory is readable at HEAD.
    #[tokio::test]
    async fn history_round_trip_restores_group_with_full_repo() {
        let id = Uuid::now_v7();
        let src = ScratchHome::new().await.expect("src home");
        let seeded = src.seed_group("origin").await.expect("seed");
        let entry = src.groups().get(&seeded.group_id).await.expect("entry");
        import_memory(
            src.backend(),
            &entry.handle,
            "note",
            &memory_doc(id, "Body."),
            None,
            src.author(),
            false,
        )
        .await
        .expect("seed memory");

        let entry = src.groups().get(&seeded.group_id).await.expect("entry");
        let mut buf = Vec::new();
        export_archive(
            src.backend(),
            &[entry],
            &ExportOptions {
                mode: ArchiveMode::History,
                ..Default::default()
            },
            &mut buf,
        )
        .await
        .expect("history export");

        let dst = ScratchHome::new().await.expect("dst home");
        let report = import_archive(
            dst.backend(),
            dst.groups(),
            dst.author(),
            &buf,
            &ImportArchiveOptions::default(),
        )
        .await
        .expect("history import");

        assert_eq!(report.groups.len(), 1);
        assert!(report.groups[0].created_group, "group restored fresh");
        assert_eq!(report.groups[0].target_group_id, *seeded.group_id.as_uuid());

        let dst_entry = dst
            .groups()
            .get(&seeded.group_id)
            .await
            .expect("group restored in dst");
        let files = list_all_memory_files(dst.backend(), &dst_entry.handle, &Rev::Head)
            .await
            .expect("files");
        assert_eq!(files.len(), 1);
    }

    /// Restoring over an existing group is a no-op without
    /// `force_restore`; with it, the whole repo is overwritten.
    #[tokio::test]
    async fn history_restore_skips_existing_then_force_overwrites() {
        let id = Uuid::now_v7();
        let src = ScratchHome::new().await.expect("src home");
        let seeded = src.seed_group("origin").await.expect("seed");
        let entry = src.groups().get(&seeded.group_id).await.expect("entry");
        import_memory(
            src.backend(),
            &entry.handle,
            "note",
            &memory_doc(id, "Body."),
            None,
            src.author(),
            false,
        )
        .await
        .expect("seed memory");
        let entry = src.groups().get(&seeded.group_id).await.expect("entry");
        let mut buf = Vec::new();
        export_archive(
            src.backend(),
            &[entry],
            &ExportOptions {
                mode: ArchiveMode::History,
                ..Default::default()
            },
            &mut buf,
        )
        .await
        .expect("history export");

        let dst = ScratchHome::new().await.expect("dst home");
        import_archive(
            dst.backend(),
            dst.groups(),
            dst.author(),
            &buf,
            &ImportArchiveOptions::default(),
        )
        .await
        .expect("first restore");

        let second = import_archive(
            dst.backend(),
            dst.groups(),
            dst.author(),
            &buf,
            &ImportArchiveOptions::default(),
        )
        .await
        .expect("second restore");
        assert!(!second.groups[0].created_group);
        assert!(second.groups[0].skipped >= 1);
        assert_eq!(second.groups[0].overwritten, 0);

        let forced = import_archive(
            dst.backend(),
            dst.groups(),
            dst.author(),
            &buf,
            &ImportArchiveOptions {
                force_restore: true,
                ..Default::default()
            },
        )
        .await
        .expect("forced restore");
        assert!(forced.groups[0].overwritten >= 1);
    }

    /// Contract test for the `NativeBackend::invalidate` wiring.
    /// A repo handle cached by an earlier read must reflect a `force_restore` swap of the same group's bare repo.
    ///
    /// Behavioral, not falsifiable: see `mmcp_git::NativeBackend::invalidate`'s doc comment for why.
    /// `NativeBackend::invalidate_evicts_cached_entry` is the mechanism-level check that can fail.
    #[tokio::test]
    async fn force_restore_invalidates_cached_repo_handle() {
        let src = ScratchHome::new().await.expect("src home");
        let seeded = src.seed_group("origin").await.expect("seed");
        let entry = src.groups().get(&seeded.group_id).await.expect("entry");
        import_memory(
            src.backend(),
            &entry.handle,
            "note",
            &memory_doc(Uuid::now_v7(), "before"),
            None,
            src.author(),
            false,
        )
        .await
        .expect("seed memory");
        let entry = src.groups().get(&seeded.group_id).await.expect("entry");
        let mut buf = Vec::new();
        export_archive(
            src.backend(),
            &[entry],
            &ExportOptions {
                mode: ArchiveMode::History,
                ..Default::default()
            },
            &mut buf,
        )
        .await
        .expect("history export");

        let dst = ScratchHome::new().await.expect("dst home");
        import_archive(
            dst.backend(),
            dst.groups(),
            dst.author(),
            &buf,
            &ImportArchiveOptions::default(),
        )
        .await
        .expect("first restore");

        // Populate dst's repo_cache via a read BEFORE the swap.
        let dst_entry = dst.groups().get(&seeded.group_id).await.expect("dst entry");
        let files_before = list_all_memory_files(dst.backend(), &dst_entry.handle, &Rev::Head)
            .await
            .expect("files before");
        assert_eq!(files_before.len(), 1);
        let before_bytes = dst
            .backend()
            .read_file(&dst_entry.handle, &files_before[0].path, &Rev::Head)
            .await
            .expect("read before");
        assert!(
            std::str::from_utf8(&before_bytes)
                .unwrap()
                .contains("before")
        );

        // Change the source content, then force-restore over the SAME
        // group (same group_id) so dst's repo path is replaced in place.
        let entry = src.groups().get(&seeded.group_id).await.expect("entry");
        import_memory(
            src.backend(),
            &entry.handle,
            "note-after",
            &memory_doc(Uuid::now_v7(), "after"),
            None,
            src.author(),
            false,
        )
        .await
        .expect("second memory");
        let entry_after = src
            .groups()
            .get(&seeded.group_id)
            .await
            .expect("entry after");
        let mut buf2 = Vec::new();
        export_archive(
            src.backend(),
            &[entry_after],
            &ExportOptions {
                mode: ArchiveMode::History,
                ..Default::default()
            },
            &mut buf2,
        )
        .await
        .expect("second export");

        let forced = import_archive(
            dst.backend(),
            dst.groups(),
            dst.author(),
            &buf2,
            &ImportArchiveOptions {
                force_restore: true,
                ..Default::default()
            },
        )
        .await
        .expect("forced restore");
        assert!(forced.groups[0].overwritten >= 1);

        // The dst backend must now serve the NEW content: two memory
        // files, and the original file's body no longer says "before"
        // (the archive rebuilds file paths from memory names/ids, so
        // the safest check is total count plus a content scan).
        let dst_entry_after = dst
            .groups()
            .get(&seeded.group_id)
            .await
            .expect("dst entry after");
        let files_after = list_all_memory_files(dst.backend(), &dst_entry_after.handle, &Rev::Head)
            .await
            .expect("files after");
        assert_eq!(
            files_after.len(),
            2,
            "restored repo should carry both memories, not the stale single-file state"
        );
        let mut saw_after = false;
        for file_ref in &files_after {
            let bytes = dst
                .backend()
                .read_file(&dst_entry_after.handle, &file_ref.path, &Rev::Head)
                .await
                .expect("read after");
            let text = std::str::from_utf8(&bytes).unwrap();
            if text.contains("after") {
                saw_after = true;
            }
        }
        assert!(saw_after, "new content must be reachable post-restore");
    }

    /// The snapshot-only knobs are rejected for a history restore.
    #[tokio::test]
    async fn history_rejects_snapshot_only_options() {
        let src = ScratchHome::new().await.expect("src home");
        let seeded = src.seed_group("origin").await.expect("seed");
        let entry = src.groups().get(&seeded.group_id).await.expect("entry");
        let mut buf = Vec::new();
        export_archive(
            src.backend(),
            &[entry],
            &ExportOptions {
                mode: ArchiveMode::History,
                ..Default::default()
            },
            &mut buf,
        )
        .await
        .expect("history export");

        let dst = ScratchHome::new().await.expect("dst home");
        let err = import_archive(
            dst.backend(),
            dst.groups(),
            dst.author(),
            &buf,
            &ImportArchiveOptions {
                new_ids: true,
                ..Default::default()
            },
        )
        .await
        .expect_err("snapshot-only option must be rejected");
        assert!(matches!(err, ArchiveError::SnapshotOnlyOption { .. }));
    }

    #[tokio::test]
    async fn round_trip_recreates_group_and_preserves_identity() {
        let id = Uuid::now_v7();
        let src = ScratchHome::new().await.expect("src home");
        let seeded = src.seed_group("origin").await.expect("seed");
        let entry = src.groups().get(&seeded.group_id).await.expect("entry");
        import_memory(
            src.backend(),
            &entry.handle,
            "note",
            &memory_doc(id, "Body one."),
            None,
            src.author(),
            false,
        )
        .await
        .expect("seed memory");
        let buf = export_group(&src, seeded.group_id).await;

        let dst = ScratchHome::new().await.expect("dst home");
        let report = import_archive(
            dst.backend(),
            dst.groups(),
            dst.author(),
            &buf,
            &ImportArchiveOptions::default(),
        )
        .await
        .expect("import");

        assert_eq!(report.groups.len(), 1);
        let g = &report.groups[0];
        assert!(g.created_group, "group should be recreated");
        assert_eq!(g.created, 1);
        assert_eq!(g.target_group_id, *seeded.group_id.as_uuid());

        let dst_entry = dst
            .groups()
            .get(&seeded.group_id)
            .await
            .expect("group recreated in dst");
        let files = list_all_memory_files(dst.backend(), &dst_entry.handle, &Rev::Head)
            .await
            .expect("list");
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].id, id);
        assert_eq!(files[0].slug, "note");
    }

    #[tokio::test]
    async fn reimport_skips_identical_memories() {
        let id = Uuid::now_v7();
        let src = ScratchHome::new().await.expect("src home");
        let seeded = src.seed_group("origin").await.expect("seed");
        let entry = src.groups().get(&seeded.group_id).await.expect("entry");
        import_memory(
            src.backend(),
            &entry.handle,
            "note",
            &memory_doc(id, "Body one."),
            None,
            src.author(),
            false,
        )
        .await
        .expect("seed memory");
        let buf = export_group(&src, seeded.group_id).await;

        let dst = ScratchHome::new().await.expect("dst home");
        let first = import_archive(
            dst.backend(),
            dst.groups(),
            dst.author(),
            &buf,
            &ImportArchiveOptions::default(),
        )
        .await
        .expect("first import");
        assert_eq!(first.groups[0].created, 1);

        let second = import_archive(
            dst.backend(),
            dst.groups(),
            dst.author(),
            &buf,
            &ImportArchiveOptions::default(),
        )
        .await
        .expect("second import");
        assert_eq!(second.groups[0].created, 0);
        assert_eq!(second.groups[0].skipped, 1);
        assert!(!second.groups[0].created_group);
    }

    #[tokio::test]
    async fn idless_memory_keeps_filename_id_and_is_idempotent() {
        let home = ScratchHome::new().await.expect("home");
        let seeded = home.seed_group("origin").await.expect("seed");
        let group_id = *seeded.group_id.as_uuid();
        let memory_id = Uuid::now_v7();
        // Frontmatter without an id; the identity lives only in the
        // archive filename `<memory_id>.md`.
        let body = "+++\nname = \"n\"\ndescription = \"d\"\nkind = \"reference\"\n+++\nBody.\n";
        let buf = build_archive(group_id, &seeded.manifest, "note", memory_id, body);

        let first = import_archive(
            home.backend(),
            home.groups(),
            home.author(),
            &buf,
            &ImportArchiveOptions::default(),
        )
        .await
        .expect("first import");
        assert_eq!(first.groups[0].created, 1);

        let entry = home.groups().get(&seeded.group_id).await.expect("entry");
        let files = list_all_memory_files(home.backend(), &entry.handle, &Rev::Head)
            .await
            .expect("list");
        assert_eq!(files.len(), 1);
        assert_eq!(
            files[0].id, memory_id,
            "filename id preserved, not re-minted"
        );

        let second = import_archive(
            home.backend(),
            home.groups(),
            home.author(),
            &buf,
            &ImportArchiveOptions::default(),
        )
        .await
        .expect("second import");
        assert_eq!(second.groups[0].created, 0);
        assert_eq!(second.groups[0].skipped, 1);
        let files = list_all_memory_files(home.backend(), &entry.handle, &Rev::Head)
            .await
            .expect("list");
        assert_eq!(files.len(), 1, "no duplicate sibling minted");
    }

    #[tokio::test]
    async fn rejects_archive_with_mismatched_inner_group_manifest() {
        let src = ScratchHome::new().await.expect("src home");
        let seeded = src.seed_group("origin").await.expect("seed");
        let entry = src.groups().get(&seeded.group_id).await.expect("entry");
        import_memory(
            src.backend(),
            &entry.handle,
            "note",
            &memory_doc(Uuid::now_v7(), "Body."),
            None,
            src.author(),
            false,
        )
        .await
        .expect("seed memory");
        let mut buf = export_group(&src, seeded.group_id).await;

        // Corrupt the inner manifest so its declared group_id differs
        // from the archive directory uuid.
        let original = seeded.manifest.to_toml().expect("manifest");
        let tampered = original.replace(
            seeded.group_id.as_uuid().to_string().as_str(),
            Uuid::now_v7().to_string().as_str(),
        );
        assert_ne!(original, tampered, "manifest must contain the group id");
        buf = rewrite_group_manifest(&buf, *seeded.group_id.as_uuid(), &tampered);

        let dst = ScratchHome::new().await.expect("dst home");
        let err = import_archive(
            dst.backend(),
            dst.groups(),
            dst.author(),
            &buf,
            &ImportArchiveOptions::default(),
        )
        .await
        .expect_err("mismatched manifest must be rejected");
        assert!(matches!(err, ArchiveError::Malformed { .. }), "got {err:?}");
    }

    /// Rebuild an archive replacing one group's `.mmcp.toml` bytes.
    fn rewrite_group_manifest(bytes: &[u8], group_id: Uuid, new_manifest: &str) -> Vec<u8> {
        let manifest_path = format!("{ARCHIVE_GROUPS_DIR}/{group_id}/{MANIFEST_FILENAME}");
        let mut out = Vec::new();
        {
            let mut builder = tar::Builder::new(&mut out);
            let mut archive = tar::Archive::new(bytes);
            for entry in archive.entries().expect("entries") {
                let mut entry = entry.expect("entry");
                let path = entry.path().expect("path").to_string_lossy().into_owned();
                let mut buf = Vec::new();
                entry.read_to_end(&mut buf).expect("read");
                let data = if path == manifest_path {
                    new_manifest.as_bytes().to_vec()
                } else {
                    buf
                };
                append_entry(&mut builder, &path, &data);
            }
            builder.finish().expect("finish");
        }
        out
    }

    #[tokio::test]
    async fn differing_content_conflicts_then_overwrites() {
        let id = Uuid::now_v7();
        let home = ScratchHome::new().await.expect("home");
        let seeded = home.seed_group("origin").await.expect("seed");
        let entry = home.groups().get(&seeded.group_id).await.expect("entry");
        import_memory(
            home.backend(),
            &entry.handle,
            "note",
            &memory_doc(id, "Body one."),
            None,
            home.author(),
            false,
        )
        .await
        .expect("seed memory");
        let buf = export_group(&home, seeded.group_id).await;

        // Mutate the same memory in place so the archive copy now
        // differs from what is on disk.
        import_memory(
            home.backend(),
            &entry.handle,
            "note",
            &memory_doc(id, "Body two."),
            None,
            home.author(),
            true,
        )
        .await
        .expect("mutate memory");

        let conflict = import_archive(
            home.backend(),
            home.groups(),
            home.author(),
            &buf,
            &ImportArchiveOptions::default(),
        )
        .await
        .expect("conflict import");
        assert_eq!(conflict.groups[0].created, 0);
        assert_eq!(conflict.groups[0].skipped, 0);
        assert_eq!(conflict.groups[0].conflicts.len(), 1);
        assert_eq!(conflict.groups[0].conflicts[0].id, id);

        let overwritten = import_archive(
            home.backend(),
            home.groups(),
            home.author(),
            &buf,
            &ImportArchiveOptions {
                overwrite: true,
                ..Default::default()
            },
        )
        .await
        .expect("overwrite import");
        assert_eq!(overwritten.groups[0].overwritten, 1);

        let resolved = resolve_memory(home.backend(), &entry.handle, None, Some(id))
            .await
            .expect("resolve");
        let restored = home
            .backend()
            .read_file(&entry.handle, &resolved.path, &Rev::Head)
            .await
            .expect("read restored");
        let restored = utf8(&resolved.path, &restored).expect("utf8").to_string();
        assert!(restored.contains("Body one."), "overwrite restored v1");
    }

    #[tokio::test]
    async fn new_ids_forks_into_fresh_identities() {
        let id = Uuid::now_v7();
        let home = ScratchHome::new().await.expect("home");
        let seeded = home.seed_group("origin").await.expect("seed");
        let entry = home.groups().get(&seeded.group_id).await.expect("entry");
        import_memory(
            home.backend(),
            &entry.handle,
            "note",
            &memory_doc(id, "Body one."),
            None,
            home.author(),
            false,
        )
        .await
        .expect("seed memory");
        let buf = export_group(&home, seeded.group_id).await;

        let report = import_archive(
            home.backend(),
            home.groups(),
            home.author(),
            &buf,
            &ImportArchiveOptions {
                new_ids: true,
                ..Default::default()
            },
        )
        .await
        .expect("fork import");
        assert_eq!(report.groups[0].created, 1);

        let files = list_all_memory_files(home.backend(), &entry.handle, &Rev::Head)
            .await
            .expect("list");
        assert_eq!(files.len(), 2, "fork adds a sibling: {files:?}");
        assert!(files.iter().any(|f| f.id == id), "original survives");
        assert!(files.iter().any(|f| f.id != id), "fork has a fresh id");
    }

    #[tokio::test]
    async fn into_remaps_memories_into_target_group() {
        let id = Uuid::now_v7();
        let src = ScratchHome::new().await.expect("src home");
        let seeded = src.seed_group("origin").await.expect("seed");
        let entry = src.groups().get(&seeded.group_id).await.expect("entry");
        import_memory(
            src.backend(),
            &entry.handle,
            "note",
            &memory_doc(id, "Body one."),
            None,
            src.author(),
            false,
        )
        .await
        .expect("seed memory");
        let buf = export_group(&src, seeded.group_id).await;

        let dst = ScratchHome::new().await.expect("dst home");
        let target = dst.seed_group("target").await.expect("seed target");
        let report = import_archive(
            dst.backend(),
            dst.groups(),
            dst.author(),
            &buf,
            &ImportArchiveOptions {
                into_group: Some(target.group_id),
                ..Default::default()
            },
        )
        .await
        .expect("remap import");

        assert_eq!(report.groups.len(), 1);
        let g = &report.groups[0];
        assert_eq!(g.source_group_id, *seeded.group_id.as_uuid());
        assert_eq!(g.target_group_id, *target.group_id.as_uuid());
        assert!(!g.created_group);
        assert_eq!(g.created, 1);

        let target_entry = dst.groups().get(&target.group_id).await.expect("target");
        let files = list_all_memory_files(dst.backend(), &target_entry.handle, &Rev::Head)
            .await
            .expect("list");
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].id, id);
        assert!(
            dst.groups().get(&seeded.group_id).await.is_none(),
            "source group must not be recreated under --into",
        );
    }

    /// Count commits reachable from `HEAD` in the bare repo at `repo_path`, via
    /// `git rev-list --count HEAD` against the repo directly.
    ///
    /// No [`mmcp_git::GitBackend`]/[`NativeBackend`] primitive returns a whole-repo
    /// commit count: [`GitBackend::walk_history`] is scoped to one path's own
    /// modification history, not the repo as a whole. This is a test-only
    /// verification helper, not a change to `mmcp-git`.
    fn commit_count(repo_path: &Path) -> usize {
        let output = std::process::Command::new("git")
            .args(["rev-list", "--count", "HEAD"])
            .current_dir(repo_path)
            .output()
            .expect("git rev-list");
        assert!(output.status.success(), "git rev-list failed: {output:?}");
        String::from_utf8(output.stdout)
            .expect("utf8 count")
            .trim()
            .parse()
            .expect("integer commit count")
    }

    /// Memory count large enough that a per-memory commit loop and a single
    /// batched commit are trivially distinguishable by commit-count delta.
    const BULK_IMPORT_MEMORY_COUNT: usize = 12;

    /// Importing many memories into an EXISTING target group produces exactly ONE
    /// new commit on the target's main branch, not one per imported memory.
    /// The regression this guards: `import_one_group`'s former per-memory
    /// `import_memory`/`write_file_at_path` calls, each of which ran its own
    /// `write_commit`.
    #[tokio::test]
    async fn batched_import_of_many_memories_produces_one_commit() {
        let src = ScratchHome::new().await.expect("src home");
        let seeded = src.seed_group("origin").await.expect("seed");
        let entry = src.groups().get(&seeded.group_id).await.expect("entry");
        for i in 0..BULK_IMPORT_MEMORY_COUNT {
            import_memory(
                src.backend(),
                &entry.handle,
                &format!("note-{i}"),
                &memory_doc(Uuid::now_v7(), &format!("Body {i}.")),
                None,
                src.author(),
                false,
            )
            .await
            .expect("seed memory");
        }
        let buf = export_group(&src, seeded.group_id).await;

        let dst = ScratchHome::new().await.expect("dst home");
        // Pre-existing target group (via `--into`) so this exercises "import into
        // an existing group", not the group-creation path.
        let target = dst.seed_group("target").await.expect("seed target");
        let repo_path = dst.backend().repo_path(*target.group_id.as_uuid());
        let before = commit_count(&repo_path);

        let report = import_archive(
            dst.backend(),
            dst.groups(),
            dst.author(),
            &buf,
            &ImportArchiveOptions {
                into_group: Some(target.group_id),
                ..Default::default()
            },
        )
        .await
        .expect("batched import");

        assert_eq!(
            report.groups[0].created, BULK_IMPORT_MEMORY_COUNT as u32,
            "every memory must still be tallied as created"
        );
        let after = commit_count(&repo_path);
        assert_eq!(
            after - before,
            1,
            "importing {BULK_IMPORT_MEMORY_COUNT} memories into an existing group \
             must add exactly ONE commit, not one per memory"
        );

        let target_entry = dst.groups().get(&target.group_id).await.expect("target");
        let files = list_all_memory_files(dst.backend(), &target_entry.handle, &Rev::Head)
            .await
            .expect("list");
        assert_eq!(files.len(), BULK_IMPORT_MEMORY_COUNT);
    }

    /// Re-importing an archive whose every memory already exists identically
    /// skips them all and adds NO commit: an all-skip batch must not produce
    /// an empty commit just because the import ran.
    #[tokio::test]
    async fn reimport_of_all_identical_memories_produces_no_commit() {
        let src = ScratchHome::new().await.expect("src home");
        let seeded = src.seed_group("origin").await.expect("seed");
        let entry = src.groups().get(&seeded.group_id).await.expect("entry");
        for i in 0..BULK_IMPORT_MEMORY_COUNT {
            import_memory(
                src.backend(),
                &entry.handle,
                &format!("note-{i}"),
                &memory_doc(Uuid::now_v7(), &format!("Body {i}.")),
                None,
                src.author(),
                false,
            )
            .await
            .expect("seed memory");
        }
        let buf = export_group(&src, seeded.group_id).await;

        let dst = ScratchHome::new().await.expect("dst home");
        import_archive(
            dst.backend(),
            dst.groups(),
            dst.author(),
            &buf,
            &ImportArchiveOptions::default(),
        )
        .await
        .expect("first import");

        let repo_path = dst.backend().repo_path(*seeded.group_id.as_uuid());
        let before = commit_count(&repo_path);

        let second = import_archive(
            dst.backend(),
            dst.groups(),
            dst.author(),
            &buf,
            &ImportArchiveOptions::default(),
        )
        .await
        .expect("second import");
        assert_eq!(second.groups[0].skipped, BULK_IMPORT_MEMORY_COUNT as u32);
        assert_eq!(second.groups[0].created, 0);

        let after = commit_count(&repo_path);
        assert_eq!(
            after, before,
            "an all-skip re-import must not add an empty commit"
        );
    }

    /// The batched snapshot must find a hand-crafted existing memory (non-UUID
    /// filename) by its FRONTMATTER id, exactly like a per-memory
    /// `resolve_memory(.., None, Some(id))` scan would, not only a canonical
    /// `memories/<slug>/<uuid>.md` file. A snapshot keyed on filename stems
    /// (e.g. built from `list_all_memory_files` instead of the full
    /// `list_memory_slug_dirs` walk) would miss this file entirely and mint a
    /// duplicate identity instead of reporting the collision.
    #[tokio::test]
    async fn batched_import_finds_hand_crafted_existing_memory_by_frontmatter_id() {
        let id = Uuid::now_v7();
        let home = ScratchHome::new().await.expect("home");
        let seeded = home.seed_group("origin").await.expect("seed");
        let entry = home.groups().get(&seeded.group_id).await.expect("entry");

        // Hand-crafted target-side memory: filename stem is not a UUID, but
        // frontmatter carries the id the incoming archive entry will also carry.
        home.backend()
            .write_commit(
                &entry.handle,
                CommitSpec::mmcp_commit(
                    "seed hand-crafted memory",
                    vec![(
                        "memories/note/hand.md".to_string(),
                        Some(memory_doc(id, "Hand-crafted body.").into_bytes()),
                    )],
                    &home.author().name,
                    &home.author().email,
                ),
            )
            .await
            .expect("seed hand-crafted memory");

        let group_id = *seeded.group_id.as_uuid();
        let buf = build_archive(
            group_id,
            &seeded.manifest,
            "note",
            id,
            &memory_doc(id, "Archived body, differs from hand-crafted."),
        );

        let report = import_archive(
            home.backend(),
            home.groups(),
            home.author(),
            &buf,
            &ImportArchiveOptions::default(),
        )
        .await
        .expect("import");

        assert_eq!(
            report.groups[0].created, 0,
            "must not mint a duplicate sibling under the same id"
        );
        assert_eq!(
            report.groups[0].conflicts.len(),
            1,
            "differing content at the same id must surface as a conflict"
        );
        assert_eq!(report.groups[0].conflicts[0].id, id);

        let files = list_all_memory_files(home.backend(), &entry.handle, &Rev::Head)
            .await
            .expect("list");
        assert_eq!(
            files.len(),
            0,
            "the hand-crafted file has a non-UUID filename and is invisible to \
             list_all_memory_files, confirming it was found by the frontmatter-id \
             snapshot instead"
        );
    }

    #[tokio::test]
    async fn select_memory_slugs_imports_only_the_chosen_memory() {
        let src = ScratchHome::new().await.expect("src");
        let seeded = src.seed_group("origin").await.expect("seed");
        let entry = src.groups().get(&seeded.group_id).await.expect("entry");
        for (slug, body) in [("keep", "K"), ("drop", "D")] {
            import_memory(
                src.backend(),
                &entry.handle,
                slug,
                &memory_doc(Uuid::now_v7(), body),
                None,
                src.author(),
                false,
            )
            .await
            .expect("seed memory");
        }
        let buf = export_group(&src, seeded.group_id).await;

        let dst = ScratchHome::new().await.expect("dst");
        let report = import_archive(
            dst.backend(),
            dst.groups(),
            dst.author(),
            &buf,
            &ImportArchiveOptions {
                filter: MemoryFilter {
                    slugs: vec!["keep".to_string()],
                    ..Default::default()
                },
                ..Default::default()
            },
        )
        .await
        .expect("import");
        assert_eq!(report.groups[0].created, 1);

        let dst_entry = dst.groups().get(&seeded.group_id).await.expect("group");
        let files = list_all_memory_files(dst.backend(), &dst_entry.handle, &Rev::Head)
            .await
            .expect("list");
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].slug, "keep");
    }

    #[tokio::test]
    async fn select_groups_imports_only_the_chosen_group() {
        let src = ScratchHome::new().await.expect("src");
        let alpha = src.seed_group("alpha").await.expect("alpha");
        let beta = src.seed_group("beta").await.expect("beta");
        let alpha_entry = src
            .groups()
            .get(&alpha.group_id)
            .await
            .expect("alpha entry");
        let beta_entry = src.groups().get(&beta.group_id).await.expect("beta entry");
        import_memory(
            src.backend(),
            &alpha_entry.handle,
            "a",
            &memory_doc(Uuid::now_v7(), "A"),
            None,
            src.author(),
            false,
        )
        .await
        .expect("a");
        import_memory(
            src.backend(),
            &beta_entry.handle,
            "b",
            &memory_doc(Uuid::now_v7(), "B"),
            None,
            src.author(),
            false,
        )
        .await
        .expect("b");
        let mut buf = Vec::new();
        export_archive(
            src.backend(),
            &[alpha_entry, beta_entry],
            &ExportOptions::default(),
            &mut buf,
        )
        .await
        .expect("export both");

        let dst = ScratchHome::new().await.expect("dst");
        let report = import_archive(
            dst.backend(),
            dst.groups(),
            dst.author(),
            &buf,
            &ImportArchiveOptions {
                select_groups: vec![beta.group_id.as_uuid().to_string()],
                ..Default::default()
            },
        )
        .await
        .expect("import");
        assert_eq!(report.groups.len(), 1);
        assert_eq!(report.groups[0].source_group_id, *beta.group_id.as_uuid());
        assert!(
            dst.groups().get(&alpha.group_id).await.is_none(),
            "alpha must not be imported",
        );
        assert!(
            dst.groups().get(&beta.group_id).await.is_some(),
            "beta must be imported",
        );
    }

    #[tokio::test]
    async fn list_archive_reports_groups_and_memory_slugs() {
        let src = ScratchHome::new().await.expect("src");
        let seeded = src.seed_group("origin").await.expect("seed");
        let entry = src.groups().get(&seeded.group_id).await.expect("entry");
        for slug in ["alpha", "beta"] {
            import_memory(
                src.backend(),
                &entry.handle,
                slug,
                &memory_doc(Uuid::now_v7(), "B"),
                None,
                src.author(),
                false,
            )
            .await
            .expect("seed");
        }
        let buf = export_group(&src, seeded.group_id).await;

        let listing = list_archive(&buf).expect("listing");
        assert_eq!(listing.len(), 1);
        assert_eq!(listing[0].group_id, *seeded.group_id.as_uuid());
        assert_eq!(listing[0].slug, "origin");
        assert_eq!(
            listing[0].memory_slugs,
            vec!["alpha".to_string(), "beta".to_string()]
        );
    }

    /// Restoring an archived memory whose body sits far above the write-time result ceiling still succeeds.
    /// Archive restore never refuses existing content.
    #[tokio::test]
    async fn restore_of_an_over_ceiling_memory_succeeds() {
        let home = ScratchHome::new().await.expect("home");
        let seeded = home.seed_group("origin").await.expect("seed");
        let group_id = *seeded.group_id.as_uuid();
        let memory_id = Uuid::now_v7();
        let oversized_body = "a".repeat(60_000);
        let body = format!(
            "+++\nid = \"{memory_id}\"\nname = \"n\"\ndescription = \"d\"\nkind = \"reference\"\n+++\n{oversized_body}\n"
        );
        let buf = build_archive(group_id, &seeded.manifest, "huge", memory_id, &body);

        let report = import_archive(
            home.backend(),
            home.groups(),
            home.author(),
            &buf,
            &ImportArchiveOptions::default(),
        )
        .await
        .expect("restoring an over-ceiling memory must not be refused");
        assert_eq!(report.groups[0].created, 1);
    }
}
