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

use tauri::{AppHandle, State};

use crate::commands::sync::SyncStatusDto;
use crate::error::GuiResult;
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
