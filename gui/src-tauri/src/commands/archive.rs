//! Archive export / import Tauri commands.
//!
//! Thin wrappers over `mmcp_store::{export_archive, import_archive}`:
//! the backend owns the tar packing and store replay, this layer owns
//! the native file dialogs (parented to the main window like
//! `workspace::pick_directory`), the protected-group confirmation, and
//! the `mirror:changed` refresh nudge after an import lands.

use mmcp_core::id::GroupId;
use mmcp_store::{ArchiveManifest, ExportOptions, ImportArchiveOptions};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};

use crate::commands::sync::MIRROR_CHANGED_EVENT;
use crate::error::{GuiError, GuiResult};
use crate::state::AppState;

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

/// Export groups to a portable archive chosen via a native save
/// dialog. An empty `group_ids` exports every mirrored group. Returns
/// `None` when the operator dismisses the dialog.
#[tauri::command]
pub async fn export_archive(
    app: AppHandle,
    state: State<'_, AppState>,
    group_ids: Vec<String>,
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

    let main = app
        .get_webview_window("main")
        .ok_or_else(|| GuiError::Other("main window is not available".into()))?;
    let suggested = if gzip {
        "mmcp-export.tar.gz"
    } else {
        "mmcp-export.tar"
    };
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .set_title("Export mmcp archive")
        .set_file_name(suggested)
        .set_parent(&main)
        .save_file(move |picked| {
            let _ = tx.send(picked);
        });
    let picked = rx
        .await
        .map_err(|e| GuiError::Other(format!("dialog channel: {e}")))?;
    let Some(target) = picked else {
        return Ok(None);
    };
    let path = target
        .into_path()
        .map_err(|e| GuiError::Other(format!("dialog path: {e}")))?;

    let file = std::fs::File::create(&path)
        .map_err(|e| GuiError::Other(format!("creating archive {}: {e}", path.display())))?;
    let manifest =
        mmcp_store::export_archive(
            &state.backend,
            &selected,
            &ExportOptions {
                gzip,
                ..Default::default()
            },
            file,
        )
        .await?;

    Ok(Some(ExportArchiveReportDto {
        output: path.to_string_lossy().into_owned(),
        group_count: manifest.groups.len(),
        memory_count: manifest.total_memory_count(),
    }))
}

/// Import an archive chosen via a native open dialog, recreating its
/// groups. `into_group` remaps every memory into one existing group.
/// Protected target groups prompt a confirmation dialog. Returns
/// `None` when the operator dismisses either dialog.
#[tauri::command]
pub async fn import_archive(
    app: AppHandle,
    state: State<'_, AppState>,
    into_group: Option<String>,
    overwrite: bool,
    new_ids: bool,
) -> GuiResult<Option<ImportArchiveReportDto>> {
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
    let Some(source) = picked else {
        return Ok(None);
    };
    let path = source
        .into_path()
        .map_err(|e| GuiError::Other(format!("dialog path: {e}")))?;

    let bytes = std::fs::read(&path)
        .map_err(|e| GuiError::Other(format!("reading archive {}: {e}", path.display())))?;
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
        ..Default::default()
    };
    let report =
        mmcp_store::import_archive(&state.backend, &state.index, &author, &bytes, &options).await?;

    // A full-mirror refresh: recreated groups and replayed memories
    // are not all under one group id, so a null target makes the
    // frontend re-list everything.
    let _ = app.emit(MIRROR_CHANGED_EVENT, serde_json::json!({ "group_id": null }));

    Ok(Some(ImportArchiveReportDto {
        input: path.to_string_lossy().into_owned(),
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

/// Confirm via a native dialog when an import would write into an
/// existing protected group. Returns `true` to proceed, `false` when
/// the operator declines. Imports that touch no protected group skip
/// the prompt entirely.
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
