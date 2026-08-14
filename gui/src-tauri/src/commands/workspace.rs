//! Workspace commands — runtime control over the "reference point"
//! directory that anchors `.mmcp.toml` discovery.
//!
//! The frontend persists `reference_point` via the normal settings
//! round-trip (`save_settings`). This command then asks the backend
//! to reload `ProjectConfig::sync` against the new path, rebuild
//! the `SyncBundle`, and respawn the reachability probe so the
//! status bar reflects the new server without requiring an app
//! restart.

use std::path::PathBuf;

use tauri::{AppHandle, Manager, State};
use tauri_plugin_dialog::DialogExt;

use crate::commands::sync::SyncStatusDto;
use crate::error::{GuiDialogError, GuiResult};
use crate::probe_loop;
use crate::state::AppState;

fn normalise(path: Option<String>) -> Option<PathBuf> {
    let raw = path?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let candidate = PathBuf::from(trimmed);
    candidate.is_dir().then_some(candidate)
}

/// Open a native folder picker parented to the main window,
/// regardless of which webview dispatched the command. Keeps the
/// dialog visually anchored to the primary app surface so the
/// Settings child window doesn't end up with a dialog that looks
/// disconnected from the work the user is doing.
#[tauri::command]
pub async fn pick_directory(
    app: AppHandle,
    default_path: Option<String>,
    title: Option<String>,
) -> GuiResult<Option<String>> {
    let main = app
        .get_webview_window("main")
        .ok_or(GuiDialogError::NoMainWindow)?;

    let mut builder = app.dialog().file();
    if let Some(t) = title {
        builder = builder.set_title(t);
    }
    if let Some(p) = default_path {
        let candidate = PathBuf::from(&p);
        if candidate.exists() {
            builder = builder.set_directory(candidate);
        }
    }
    builder = builder.set_parent(&main);

    let (tx, rx) = tokio::sync::oneshot::channel();
    builder.pick_folder(move |picked| {
        let _ = tx.send(picked);
    });
    let picked = rx.await.map_err(|_| GuiDialogError::ChannelClosed)?;

    Ok(picked.and_then(|p| {
        p.into_path()
            .ok()
            .map(|pb| pb.to_string_lossy().into_owned())
    }))
}

#[tauri::command]
pub async fn set_reference_point(
    app: AppHandle,
    state: State<'_, AppState>,
    path: Option<String>,
) -> GuiResult<SyncStatusDto> {
    let resolved = normalise(path);
    let new_url = state.rebuild_sync(resolved.as_deref()).await?;

    // Swap probes. An aborted task just drops its future; no need to
    // await it — the old `get_manifest()` request may still finish
    // on the wire but its result is discarded.
    let mut probe = state.probe.lock().await;
    if let Some(old) = probe.take() {
        old.abort();
    }
    if let Some(url) = new_url.clone() {
        let fresh = tauri::async_runtime::spawn(probe_loop(app.clone(), url));
        *probe = Some(fresh);
    }

    Ok(SyncStatusDto {
        configured: new_url.is_some(),
        server_url: new_url,
    })
}
