//! Archive export / import Tauri commands.
//!
//! Thin wrappers over `mmcp_store` archive primitives. This layer owns
//! the native file dialogs (parented to the main window), the
//! protected-group confirmation, and the `mirror:changed` refresh
//! nudge after an import lands. The selection dialog drives the flow:
//! it lists local groups/memories for export and inspects a chosen
//! archive for import, then calls export / import with the picks.

use mmcp_core::id::GroupId;
use mmcp_store::{ArchiveManifest, ExportOptions, ImportArchiveOptions, MemoryFilter, parse_memory_kind};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};

use crate::commands::sync::MIRROR_CHANGED_EVENT;
use crate::error::{GuiError, GuiResult};
use crate::state::AppState;

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
            parse_memory_kind(value).ok_or_else(|| GuiError::Other(format!("unknown kind '{value}'")))
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
        return Err(GuiError::Other("no groups to export".into()));
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
#[tauri::command]
pub async fn pick_import_path(app: AppHandle) -> GuiResult<Option<String>> {
    let main = app
        .get_webview_window("main")
        .ok_or_else(|| GuiError::Other("main window is not available".into()))?;
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .set_title("Import mmcp archive")
        .set_parent(&main)
        .pick_file(move |picked| {
            let _ = tx.send(picked);
        });
    let picked = rx
        .await
        .map_err(|e| GuiError::Other(format!("dialog channel: {e}")))?;
    Ok(picked.and_then(|p| p.into_path().ok().map(|pb| pb.to_string_lossy().into_owned())))
}

/// Enumerate an archive's groups and the memory slugs each carries, so
/// the import dialog can offer group- and memory-level selection.
#[tauri::command]
pub async fn inspect_archive(input: String) -> GuiResult<Vec<ArchiveGroupListingDto>> {
    let bytes = std::fs::read(&input)
        .map_err(|e| GuiError::Other(format!("reading archive {input}: {e}")))?;
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
pub async fn local_tags(state: State<'_, AppState>, group_ids: Vec<String>) -> GuiResult<Vec<String>> {
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
    let bytes = std::fs::read(&input)
        .map_err(|e| GuiError::Other(format!("reading archive {input}: {e}")))?;
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
    let _ = app.emit(MIRROR_CHANGED_EVENT, serde_json::json!({ "group_id": null }));

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
async fn pick_save_path(
    app: &AppHandle,
    suggested_name: &str,
) -> GuiResult<Option<std::path::PathBuf>> {
    let main = app
        .get_webview_window("main")
        .ok_or_else(|| GuiError::Other("main window is not available".into()))?;
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .set_title("Export mmcp archive")
        .set_file_name(suggested_name)
        .set_parent(&main)
        .save_file(move |picked| {
            let _ = tx.send(picked);
        });
    let picked = rx
        .await
        .map_err(|e| GuiError::Other(format!("dialog channel: {e}")))?;
    match picked {
        Some(target) => Ok(Some(
            target
                .into_path()
                .map_err(|e| GuiError::Other(format!("dialog path: {e}")))?,
        )),
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
        .map_err(|e| GuiError::Other(format!("dialog channel: {e}")))
}
