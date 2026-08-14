//! Shared tracker-layer machinery.
//!
//! `features` and `issues` are structurally near-identical CRUD surfaces,
//! over two distinct memory kinds (`Feature` and `Issue`),
//! that deliberately keep distinct status vocabularies: two systems, two purposes.
//!
//! What *is* shared is the surrounding plumbing:
//! the per-group ticket counter, cross-reference composition, slug-rename git planning,
//! and the listing/notes-channel shape that decides which records survive a filter,
//! and how a corrupt-on-disk memory gets reported instead of silently dropped.
//!
//! The helper lives in its own concern-named module so neither tracker surface owns it.

use mmcp_core::conventions::{MEMORIES_DIR, MEMORY_EXTENSION, memory_path};
use mmcp_core::memory::{MemoryFile, MemoryRef, Status};
use mmcp_git::{GitBackend, NativeBackend, Rev};
use uuid::Uuid;

use crate::diagnostics::Finding;
use crate::groups::GroupEntry;
use crate::memory::{ImportError, list_memory_slug_dirs};

/// Compute the next ticket number for the group.
///
/// Reads the frontmatter of every memory under `memories/`,
/// looks at `feature.number` and `issue.number`,
/// and returns one more than the maximum observed value.
/// Returns `1` for an empty group.
/// Nested slug paths are walked recursively via [`list_memory_slug_dirs`].
///
/// Errors only on a hard list / read failure on the underlying git tree;
/// per-memory parse errors are ignored so a single malformed file does not stall the counter.
pub async fn next_ticket_number(
    backend: &NativeBackend,
    entry: &GroupEntry,
) -> Result<u32, ImportError> {
    let rev = Rev::head();
    let slug_dirs = list_memory_slug_dirs(backend, &entry.handle, &rev)
        .await
        .map_err(ImportError::Git)?;
    let mut max = 0u32;
    for slug_dir in slug_dirs {
        for filename in &slug_dir.filenames {
            let path = format!("{}/{filename}", slug_dir.dir);
            let Ok(bytes) = backend.read_file(&entry.handle, &path, &rev).await else {
                continue;
            };
            let Ok(text) = std::str::from_utf8(&bytes) else {
                continue;
            };
            let Ok(mf) = MemoryFile::parse(text) else {
                continue;
            };
            if let Some(meta) = mf.frontmatter.feature.as_ref()
                && let Some(n) = meta.number
            {
                max = max.max(n);
            }
            if let Some(meta) = mf.frontmatter.issue.as_ref()
                && let Some(n) = meta.number
            {
                max = max.max(n);
            }
        }
    }
    Ok(max + 1)
}

/// Compose the merged cross-reference list for an update: removals
/// apply first, then each addition replaces any existing ref to the
/// same target. Shared between `update_feature` and `update_issue`,
/// which resolved `refs_remove` / `refs_add` identically before
/// this extraction.
pub(crate) fn compose_refs(
    current: Vec<MemoryRef>,
    remove: Option<&[Uuid]>,
    add: Option<&[MemoryRef]>,
) -> Vec<MemoryRef> {
    let mut out = current;
    if let Some(remove) = remove {
        out.retain(|r| !remove.contains(&r.target));
    }
    if let Some(add) = add {
        for new in add {
            out.retain(|r| r.target != new.target);
            out.push(new.clone());
        }
    }
    out
}

/// Decide whether a tracker record carrying `status` survives a listing filter.
/// Shared precedence between `list_features` and `list_issues`:
///
/// 1. `status_filter = Some(x)` wins over `show_all`: an explicit selector always includes a matching record.
/// 2. `status_filter = None`, `show_all = true`: every record.
/// 3. `status_filter = None`, `show_all = false`:
///    hide whatever the status's own [`Status::is_default_hidden`] marks terminal-ish,
///    so each tracker kind declares its own default visibility once instead of every call site re-deciding it.
pub(crate) fn listing_keeps_status<S: Status>(
    status: S,
    status_filter: Option<S>,
    show_all: bool,
) -> bool {
    match status_filter {
        Some(want) => status == want,
        None if show_all => true,
        None => !status.is_default_hidden(),
    }
}

/// Build the `frontmatter_parse_failed` [`Finding`] both `list_features` and `list_issues` emit,
/// for a memory whose frontmatter failed to parse,
/// so a corrupt-on-disk tracker memory is reported instead of silently vanishing from the listing.
///
/// `pub` (not `pub(crate)`): `mmcp-client`'s generic `list_memories`/`search_memories` tools reuse this shape,
/// for the same parse-failure-fabrication pattern outside the tracker kinds,
/// so every caller across the workspace reports one `frontmatter_parse_failed` code,
/// with one message format instead of drifting per crate.
pub fn parse_failed_finding(
    group: &str,
    slug: &str,
    err: &mmcp_core::memory::MemoryParseError,
) -> Finding {
    Finding {
        group: group.to_string(),
        slug: Some(slug.to_string()),
        severity: "error",
        code: "frontmatter_parse_failed",
        message: format!("frontmatter parse failed: {err}"),
    }
}

/// Git move-list produced by [`plan_slug_rename`], ready for `CommitSpec::mmcp_commit`.
pub(crate) type PlannedRename = Vec<(String, Option<Vec<u8>>)>;

/// Walk every memory file under `old_slug`'s two-level directory,
/// parse each, and stage a git move to `new_slug` in one batch.
/// Shared git plumbing between `rename_feature` and `rename_issue`;
/// only the ownership check differs between the two kinds,
/// so it is supplied by the caller as `reject_foreign`:
/// given the parsed file, return `Some(err)` when the memory does not belong to the caller's tracker kind,
/// mirroring `delete_feature`/`delete_issue`'s not-a-* guard,
/// or `None` to accept it into the rename.
///
/// Errors on a hard git failure, on an empty source directory, or on the first rejected file:
/// the caller's `E` must be constructible from [`ImportError`],
/// (`#[from]` on the wrapping variant already gives every tracker error type this for free).
pub(crate) async fn plan_slug_rename<E>(
    backend: &NativeBackend,
    entry: &GroupEntry,
    old_slug: &str,
    new_slug: &str,
    reject_foreign: impl Fn(&MemoryFile) -> Option<E>,
) -> Result<PlannedRename, E>
where
    E: From<ImportError>,
{
    let old_dir = format!("{MEMORIES_DIR}/{old_slug}");
    let entries = backend
        .list_tree(&entry.handle, &old_dir, &Rev::head())
        .await
        .map_err(|e| E::from(ImportError::Git(e)))?;
    if entries.is_empty() {
        return Err(E::from(ImportError::MemoryNotFound {
            slug: Some(old_slug.to_string()),
            id: None,
        }));
    }

    let mut moves: PlannedRename = Vec::with_capacity(entries.len() * 2);
    let mut moved = 0usize;
    for filename in &entries {
        let Some(stem) = filename.strip_suffix(MEMORY_EXTENSION) else {
            continue;
        };
        let Ok(id) = Uuid::parse_str(stem) else {
            continue;
        };
        let old_path = memory_path(old_slug, id);
        let new_path = memory_path(new_slug, id);
        let bytes = backend
            .read_file(&entry.handle, &old_path, &Rev::head())
            .await
            .map_err(|e| E::from(ImportError::Git(e)))?;

        let text = String::from_utf8_lossy(&bytes).into_owned();
        let file = MemoryFile::parse(&text).map_err(|e| E::from(ImportError::Parse(e)))?;
        if let Some(err) = reject_foreign(&file) {
            return Err(err);
        }

        moves.push((new_path, Some(bytes.to_vec())));
        moves.push((old_path, None));
        moved += 1;
    }
    if moved == 0 {
        return Err(E::from(ImportError::MemoryNotFound {
            slug: Some(old_slug.to_string()),
            id: None,
        }));
    }
    Ok(moves)
}

/// Count how many memory files under `slug`'s two-level directory
/// are valid UUID-named entries. Shared between
/// `list_features_for_slug` and `list_issues_for_slug`, whose walk
/// and filter shape is identical; the per-record read stays
/// kind-specific (`read_feature` / `read_issue`) in each caller.
pub(crate) async fn count_slug_entries(
    backend: &NativeBackend,
    entry: &GroupEntry,
    slug: &str,
) -> Result<usize, ImportError> {
    let dir = format!("{MEMORIES_DIR}/{slug}");
    let filenames = backend
        .list_tree(&entry.handle, &dir, &Rev::head())
        .await
        .map_err(ImportError::Git)?;
    Ok(filenames
        .iter()
        .filter(|filename| {
            filename
                .strip_suffix(MEMORY_EXTENSION)
                .is_some_and(|stem| Uuid::parse_str(stem).is_ok())
        })
        .count())
}
