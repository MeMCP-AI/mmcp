//! Sync commands (pull / push / status snapshot).

use mmcp_sync::SyncFilter;
use serde::Serialize;
use tauri::{AppHandle, Emitter, State};

use crate::error::{GuiError, GuiResult};
use crate::state::AppState;

/// Broadcast name used whenever the on-disk mirror may have
/// changed. Frontend stores listen and silently re-sync. Payload
/// is always `{ group_id: string | null }` — null means "something
/// at the repos_root level changed, refresh globally".
pub const MIRROR_CHANGED_EVENT: &str = "mirror:changed";

#[derive(Debug, Serialize)]
pub struct SyncStatusDto {
    pub configured: bool,
    pub server_url: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct PullReportDto {
    pub updated: usize,
    pub new_groups: usize,
}

#[derive(Debug, Serialize)]
pub struct PushReportDto {
    pub pushed: usize,
}

#[tauri::command]
pub async fn sync_status(state: State<'_, AppState>) -> GuiResult<SyncStatusDto> {
    let guard = state.sync.read().await;
    Ok(SyncStatusDto {
        configured: guard.is_some(),
        server_url: guard.as_ref().map(|s| s.server_url.clone()),
    })
}

#[tauri::command]
pub async fn sync_pull(
    app: AppHandle,
    state: State<'_, AppState>,
) -> GuiResult<PullReportDto> {
    let guard = state.sync.read().await;
    let bundle = guard.as_ref().ok_or(GuiError::SyncNotConfigured)?;
    // `IndexResolver` implements both `GroupHandleResolver` and
    // `ScopeIndex`, so it plays both roles in the engine's new
    // three-arg signature. `SyncFilter::All` iterates every
    // locally-indexed group — the GUI doesn't expose a subset
    // picker yet, matching what the CLI's default pull does.
    let report = bundle
        .engine
        .pull(SyncFilter::All, &bundle.resolver, &bundle.resolver)
        .await
        .map_err(|e| GuiError::Sync(e.to_string()))?;
    // A pull touched one or more groups' refs — tell every frontend
    // listener so views refresh silently. We don't itemise which
    // groups changed because the pull report isn't per-group here;
    // null payload means "refresh what you've cached".
    let _ = app.emit(
        MIRROR_CHANGED_EVENT,
        serde_json::json!({ "group_id": null }),
    );
    Ok(PullReportDto {
        updated: report.updated.len(),
        new_groups: report.new_groups.len(),
    })
}

#[tauri::command]
pub async fn sync_push(state: State<'_, AppState>) -> GuiResult<PushReportDto> {
    let guard = state.sync.read().await;
    let bundle = guard.as_ref().ok_or(GuiError::SyncNotConfigured)?;
    // The engine no longer consumes an explicit `PendingQueue`
    // argument — filter + resolver pair does the same work, so
    // the stored queue just lives on for any future feature that
    // needs it.
    let report = bundle
        .engine
        .push(SyncFilter::All, &bundle.resolver, &bundle.resolver)
        .await
        .map_err(|e| GuiError::Sync(e.to_string()))?;
    Ok(PushReportDto {
        pushed: report.pushed.len(),
    })
}
