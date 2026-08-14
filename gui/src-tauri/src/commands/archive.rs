//! Archive export / import Tauri commands.
//!
//! Thin wrappers over `mmcp_store` archive primitives. This layer owns
//! the native file dialogs (parented to the main window), the
//! protected-group confirmation, and the `mirror:changed` refresh
//! nudge after an import lands. The selection dialog drives the flow:
//! it lists local groups/memories for export and inspects a chosen
//! archive for import, then calls export / import with the picks.

use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};

use mmcp_core::id::GroupId;
use mmcp_store::{
    ArchiveManifest, ExportOptions, ImportArchiveOptions, MemoryFilter, parse_memory_kind,
};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};

use crate::commands::sync::MIRROR_CHANGED_EVENT;
use crate::error::{GuiArchiveError, GuiResult};
use crate::state::AppState;

/// Maximum size, in bytes, `inspect_archive` / `import_archive` will
/// read into memory for a single archive file. Confinement to a
/// picked path (below) already limits *which* file can be read; this
/// bounds *how much* of it a single IPC call pulls into memory.
const MAX_ARCHIVE_READ_BYTES: u64 = 512 * 1024 * 1024;

/// Path most recently returned by the "Import mmcp archive" native
/// picker in [`pick_import_path`]. `inspect_archive` / `import_archive`
/// refuse any `input` that doesn't canonicalize to this value,
/// confining their filesystem reads to a path the operator actually
/// chose through the dialog rather than trusting an arbitrary
/// IPC-supplied string.
static LAST_PICKED_IMPORT_PATH: LazyLock<Mutex<Option<PathBuf>>> =
    LazyLock::new(|| Mutex::new(None));

/// Memory filter facets sent from the dialog's advanced panel.
#[derive(Debug, Default, Deserialize)]
pub struct ArchiveFilterDto {
    #[serde(default)]
    pub memory: Vec<String>,
    #[serde(default)]
    pub exclude_memory: Vec<String>,
    #[serde(default)]
    pub kind: Vec<String>,
    #[serde(default)]
    pub exclude_kind: Vec<String>,
    #[serde(default)]
    pub tag: Vec<String>,
    #[serde(default)]
    pub all_tags: bool,
    #[serde(default)]
    pub exclude_tag: Vec<String>,
    #[serde(default)]
    pub search: Option<String>,
    #[serde(default)]
    pub mandatory: Option<bool>,
    #[serde(default)]
    pub has_refs: Option<bool>,
}

impl ArchiveFilterDto {
    fn to_filter(&self) -> GuiResult<MemoryFilter> {
        Ok(MemoryFilter {
            slugs: self.memory.clone(),
            exclude_slugs: self.exclude_memory.clone(),
            kinds: parse_kinds(&self.kind)?,
            exclude_kinds: parse_kinds(&self.exclude_kind)?,
            tags: self.tag.clone(),
            require_all_tags: self.all_tags,
            exclude_tags: self.exclude_tag.clone(),
            search: self.search.clone(),
            mandatory: self.mandatory,
            has_refs: self.has_refs,
        })
    }
}

/// Wire string for a group scope, matching the frontend `GroupScope`.
fn scope_str(scope: mmcp_core::manifest::GroupScope) -> &'static str {
    use mmcp_core::manifest::GroupScope;
    match scope {
        GroupScope::Global => "global",
        GroupScope::Shared => "shared",
        GroupScope::Project => "project",
    }
}

fn parse_kinds(values: &[String]) -> GuiResult<Vec<mmcp_core::memory::MemoryKind>> {
    values
        .iter()
        .map(|value| {
            parse_memory_kind(value)
                .ok_or_else(|| GuiArchiveError::UnknownKind(value.clone()).into())
        })
        .collect()
}

/// One archived group's contents for the import selection dialog.
#[derive(Debug, Serialize)]
pub struct ArchiveGroupListingDto {
    pub group_id: String,
    pub slug: String,
    pub scope: String,
    pub memory_slugs: Vec<String>,
    pub tags: Vec<String>,
}

/// Summary returned to the frontend after an export.
#[derive(Debug, Serialize)]
pub struct ExportArchiveReportDto {
    pub output: String,
    pub group_count: usize,
    pub memory_count: u64,
}

/// Per-group outcome of an import, flattened for the frontend.
#[derive(Debug, Serialize)]
pub struct GroupImportOutcomeDto {
    pub source_group_id: String,
    pub target_group_id: String,
    pub slug: String,
    pub created_group: bool,
    pub created: u32,
    pub overwritten: u32,
    pub skipped: u32,
    pub conflicts: usize,
}

/// Summary returned to the frontend after an import.
#[derive(Debug, Serialize)]
pub struct ImportArchiveReportDto {
    pub input: String,
    pub groups: Vec<GroupImportOutcomeDto>,
}

/// Export the chosen groups (and optionally a memory-slug subset) to an
/// archive picked via a native save dialog. An empty `group_ids`
/// exports every mirrored group. Returns `None` when the operator
/// dismisses the dialog.
#[tauri::command]
pub async fn export_archive(
    app: AppHandle,
    state: State<'_, AppState>,
    group_ids: Vec<String>,
    filter: ArchiveFilterDto,
    gzip: bool,
) -> GuiResult<Option<ExportArchiveReportDto>> {
    let selected = if group_ids.is_empty() {
        state.index.list().await
    } else {
        let mut out = Vec::with_capacity(group_ids.len());
        for id in &group_ids {
            out.push(mmcp_store::resolve_group(&state.index, id).await?);
        }
        out
    };
    if selected.is_empty() {
        return Err(GuiArchiveError::NoGroupsSelected.into());
    }

    let suggested = if gzip {
        "mmcp-export.tar.gz"
    } else {
        "mmcp-export.tar"
    };
    let Some(path) = pick_save_path(&app, suggested).await? else {
        return Ok(None);
    };

    let options = ExportOptions {
        gzip,
        filter: filter.to_filter()?,
        ..Default::default()
    };
    let manifest =
        mmcp_store::export_archive_to_path(&state.backend, &selected, &options, &path).await?;

    Ok(Some(ExportArchiveReportDto {
        output: path.to_string_lossy().into_owned(),
        group_count: manifest.groups.len(),
        memory_count: manifest.total_memory_count(),
    }))
}

/// Open a native picker for an archive to import and return its path,
/// or `None` if the operator dismisses the dialog. The frontend then
/// inspects the archive before committing to an import.
///
/// The canonicalized path is also stashed as the one path
/// `inspect_archive` / `import_archive` will accept, so those commands
/// never trust an arbitrary IPC-supplied filesystem path (see
/// [`read_confined_archive`]).
#[tauri::command]
pub async fn pick_import_path(app: AppHandle) -> GuiResult<Option<String>> {
    let main = app
        .get_webview_window("main")
        .ok_or(GuiArchiveError::NoMainWindow)?;
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .set_title("Import mmcp archive")
        .set_parent(&main)
        .pick_file(move |picked| {
            let _ = tx.send(picked);
        });
    let picked = rx.await.map_err(|_| GuiArchiveError::DialogChannelClosed)?;
    let Some(picked) = picked else {
        return Ok(None);
    };
    let path = picked
        .into_path()
        .map_err(|e| GuiArchiveError::DialogPathUnusable(e.to_string()))?;
    let canonical = path
        .canonicalize()
        .map_err(|source| GuiArchiveError::Read {
            path: path.to_string_lossy().into_owned(),
            source,
        })?;
    *last_picked_import_path() = Some(canonical.clone());
    Ok(Some(canonical.to_string_lossy().into_owned()))
}

/// Enumerate an archive's groups and the memory slugs each carries, so
/// the import dialog can offer group- and memory-level selection.
#[tauri::command]
pub async fn inspect_archive(input: String) -> GuiResult<Vec<ArchiveGroupListingDto>> {
    let bytes = read_confined_archive(&input)?;
    let listing = mmcp_store::list_archive(&bytes)?;
    Ok(listing
        .into_iter()
        .map(|g| ArchiveGroupListingDto {
            group_id: g.group_id.to_string(),
            slug: g.slug,
            scope: scope_str(g.scope).to_string(),
            memory_slugs: g.memory_slugs,
            tags: g.tags,
        })
        .collect())
}

/// Distinct tags across the chosen local groups (empty = every group),
/// for the export dialog's tag autocomplete.
#[tauri::command]
pub async fn local_tags(
    state: State<'_, AppState>,
    group_ids: Vec<String>,
) -> GuiResult<Vec<String>> {
    let groups = if group_ids.is_empty() {
        state.index.list().await
    } else {
        let mut out = Vec::with_capacity(group_ids.len());
        for id in &group_ids {
            out.push(mmcp_store::resolve_group(&state.index, id).await?);
        }
        out
    };
    Ok(mmcp_store::collect_group_tags(&state.backend, &groups).await?)
}

/// Import a previously-picked archive with the dialog's selection.
/// `only_groups` / `only_memory_slugs` restrict what is replayed;
/// `into_group` remaps into one existing group. Protected target
/// groups prompt for confirmation. Returns `None` if the operator
/// declines a protected write.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn import_archive(
    app: AppHandle,
    state: State<'_, AppState>,
    input: String,
    only_groups: Vec<String>,
    filter: ArchiveFilterDto,
    into_group: Option<String>,
    overwrite: bool,
    new_ids: bool,
) -> GuiResult<Option<ImportArchiveReportDto>> {
    let bytes = read_confined_archive(&input)?;
    let manifest = mmcp_store::inspect_archive(&bytes)?;

    let into = match &into_group {
        Some(query) => {
            let entry = mmcp_store::resolve_group(&state.index, query).await?;
            Some(GroupId::from_uuid(entry.handle.group_id))
        }
        None => None,
    };

    if !confirm_protected(&app, &state, &manifest, into).await? {
        return Ok(None);
    }

    let author = state.author.read().await.clone();
    let options = ImportArchiveOptions {
        into_group: into,
        overwrite,
        new_ids,
        allow_protected: true,
        select_groups: only_groups,
        filter: filter.to_filter()?,
        force_restore: false,
    };
    let report =
        mmcp_store::import_archive(&state.backend, &state.index, &author, &bytes, &options).await?;

    // Recreated groups and replayed memories span several group ids, so
    // a null target makes the frontend re-list everything.
    let _ = app.emit(
        MIRROR_CHANGED_EVENT,
        serde_json::json!({ "group_id": null }),
    );

    Ok(Some(ImportArchiveReportDto {
        input,
        groups: report
            .groups
            .iter()
            .map(|g| GroupImportOutcomeDto {
                source_group_id: g.source_group_id.to_string(),
                target_group_id: g.target_group_id.to_string(),
                slug: g.slug.clone(),
                created_group: g.created_group,
                created: g.created,
                overwritten: g.overwritten,
                skipped: g.skipped,
                conflicts: g.conflicts.len(),
            })
            .collect(),
    }))
}

/// Open a native save picker parented to the main window.
async fn pick_save_path(app: &AppHandle, suggested_name: &str) -> GuiResult<Option<PathBuf>> {
    let main = app
        .get_webview_window("main")
        .ok_or(GuiArchiveError::NoMainWindow)?;
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .set_title("Export mmcp archive")
        .set_file_name(suggested_name)
        .set_parent(&main)
        .save_file(move |picked| {
            let _ = tx.send(picked);
        });
    let picked = rx.await.map_err(|_| GuiArchiveError::DialogChannelClosed)?;
    match picked {
        Some(target) => {
            Ok(Some(target.into_path().map_err(|e| {
                GuiArchiveError::DialogPathUnusable(e.to_string())
            })?))
        }
        None => Ok(None),
    }
}

/// Confirm via a native dialog when an import would write into an
/// existing protected group. Returns `true` to proceed, `false` when
/// the operator declines. Imports touching no protected group skip the
/// prompt entirely.
async fn confirm_protected(
    app: &AppHandle,
    state: &AppState,
    manifest: &ArchiveManifest,
    into: Option<GroupId>,
) -> GuiResult<bool> {
    let mut protected: Vec<String> = Vec::new();
    if let Some(group_id) = into {
        if let Some(entry) = state.index.get(&group_id).await
            && entry.manifest.protected
        {
            protected.push(entry.manifest.slug.clone());
        }
    } else {
        for group_meta in &manifest.groups {
            if let Ok(entry) =
                mmcp_store::resolve_group(&state.index, &group_meta.group_id.to_string()).await
                && entry.manifest.protected
            {
                protected.push(entry.manifest.slug.clone());
            }
        }
    }
    if protected.is_empty() {
        return Ok(true);
    }

    let message = format!(
        "This import writes into protected group(s): {}. Continue?",
        protected.join(", "),
    );
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog()
        .message(message)
        .title("Protected group")
        .kind(MessageDialogKind::Warning)
        .buttons(MessageDialogButtons::OkCancel)
        .show(move |confirmed| {
            let _ = tx.send(confirmed);
        });
    rx.await
        .map_err(|_| GuiArchiveError::DialogChannelClosed.into())
}

/// Read `input` after confirming it is the path most recently
/// returned by [`pick_import_path`] and that its size is within
/// [`MAX_ARCHIVE_READ_BYTES`]. Confines archive reads to a path the
/// operator actually selected through the native file picker rather
/// than trusting an arbitrary IPC-supplied string, and caps how much
/// of it a single call pulls into memory.
fn read_confined_archive(input: &str) -> GuiResult<Vec<u8>> {
    let candidate = PathBuf::from(input);
    let canonical = candidate
        .canonicalize()
        .map_err(|source| GuiArchiveError::Read {
            path: input.to_string(),
            source,
        })?;

    let picked = last_picked_import_path();
    ensure_path_was_picked(input, &canonical, picked.as_deref())?;
    drop(picked);

    let metadata = std::fs::metadata(&canonical).map_err(|source| GuiArchiveError::Read {
        path: input.to_string(),
        source,
    })?;
    ensure_within_size_cap(input, metadata.len(), MAX_ARCHIVE_READ_BYTES)?;

    std::fs::read(&canonical).map_err(|source| {
        GuiArchiveError::Read {
            path: input.to_string(),
            source,
        }
        .into()
    })
}

/// Lock [`LAST_PICKED_IMPORT_PATH`]. The lock is only ever held across
/// a few non-blocking statements (never across an `.await`), so
/// poisoning would mean an earlier holder panicked mid-critical-section:
/// a bug elsewhere in this module, not a condition callers recover from.
fn last_picked_import_path() -> std::sync::MutexGuard<'static, Option<PathBuf>> {
    LAST_PICKED_IMPORT_PATH
        .lock()
        .expect("archive picker mutex poisoned by an earlier panic")
}

/// `input` (the raw string an IPC caller supplied) is only accepted
/// when its canonicalized form matches the path the operator actually
/// chose through the native picker.
fn ensure_path_was_picked(
    input: &str,
    canonical: &Path,
    picked: Option<&Path>,
) -> Result<(), GuiArchiveError> {
    if picked == Some(canonical) {
        Ok(())
    } else {
        Err(GuiArchiveError::PathNotPicked {
            path: input.to_string(),
        })
    }
}

/// `size` bytes at `path` must not exceed `max`.
fn ensure_within_size_cap(path: &str, size: u64, max: u64) -> Result<(), GuiArchiveError> {
    if size > max {
        Err(GuiArchiveError::TooLarge {
            path: path.to_string(),
            size,
            max,
        })
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_not_picked_is_refused() {
        let canonical = PathBuf::from("/mirror/archive.tar");
        let err = ensure_path_was_picked("archive.tar", &canonical, None).unwrap_err();
        assert!(matches!(err, GuiArchiveError::PathNotPicked { path } if path == "archive.tar"));
    }

    #[test]
    fn path_picked_but_different_is_refused() {
        let canonical = PathBuf::from("/mirror/archive.tar");
        let other = PathBuf::from("/mirror/other.tar");
        let err = ensure_path_was_picked("archive.tar", &canonical, Some(&other)).unwrap_err();
        assert!(matches!(err, GuiArchiveError::PathNotPicked { .. }));
    }

    #[test]
    fn path_matching_the_picked_path_is_accepted() {
        let canonical = PathBuf::from("/mirror/archive.tar");
        ensure_path_was_picked("archive.tar", &canonical, Some(&canonical)).unwrap();
    }

    #[test]
    fn size_over_the_cap_is_refused() {
        let err = ensure_within_size_cap("archive.tar", 200, 100).unwrap_err();
        assert!(matches!(
            err,
            GuiArchiveError::TooLarge {
                size: 200,
                max: 100,
                ..
            }
        ));
    }

    #[test]
    fn size_within_the_cap_is_accepted() {
        ensure_within_size_cap("archive.tar", 50, 100).unwrap();
        ensure_within_size_cap("archive.tar", 100, 100).unwrap();
    }
}
