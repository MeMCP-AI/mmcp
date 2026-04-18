//! Sync commands (pull / push / status snapshot).

use serde::Serialize;
use tauri::State;

use crate::error::{GuiError, GuiResult};
use crate::state::AppState;

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
    pub drained: usize,
}

#[tauri::command]
pub async fn sync_status(state: State<'_, AppState>) -> GuiResult<SyncStatusDto> {
    Ok(SyncStatusDto {
        configured: state.sync.is_some(),
        server_url: state.sync.as_ref().map(|s| s.server_url.clone()),
    })
}

#[tauri::command]
pub async fn sync_pull(state: State<'_, AppState>) -> GuiResult<PullReportDto> {
    let bundle = state.sync.as_ref().ok_or(GuiError::SyncNotConfigured)?;
    let report = bundle
        .engine
        .pull(&bundle.resolver)
        .await
        .map_err(|e| GuiError::Sync(e.to_string()))?;
    Ok(PullReportDto {
        updated: report.updated.len(),
        new_groups: report.new_groups.len(),
    })
}

#[tauri::command]
pub async fn sync_push(state: State<'_, AppState>) -> GuiResult<PushReportDto> {
    let bundle = state.sync.as_ref().ok_or(GuiError::SyncNotConfigured)?;
    let queue = bundle.queue.lock().await;
    let report = bundle
        .engine
        .push(&queue, &bundle.resolver)
        .await
        .map_err(|e| GuiError::Sync(e.to_string()))?;
    Ok(PushReportDto {
        drained: report.drained.len(),
    })
}
