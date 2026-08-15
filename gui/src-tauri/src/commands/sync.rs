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

/// Stable [`mmcp_store::Finding::code`] for each `mmcp_sync::SyncError`
/// variant, so a caller can branch on the failure cause without
/// parsing `message`'s free-form text.
fn sync_error_code(err: &mmcp_sync::SyncError) -> &'static str {
    match err {
        mmcp_sync::SyncError::InvalidVersion(_) => "sync_invalid_version",
        mmcp_sync::SyncError::NotFound(_) => "sync_not_found",
        mmcp_sync::SyncError::Git(_) => "sync_git_error",
        mmcp_sync::SyncError::Transport(_) => "sync_transport_error",
        mmcp_sync::SyncError::Remote { .. } => "sync_remote_error",
        mmcp_sync::SyncError::Conflict { .. } => "sync_conflict",
        mmcp_sync::SyncError::PullDiverged { .. } => "sync_pull_diverged",
        mmcp_sync::SyncError::PushDiverged { .. } => "sync_push_diverged",
    }
}

/// Convert an engine report's `failed` list into [`mmcp_store::Finding`]s,
/// the workspace's one skip/failure record shape, instead of a bespoke
/// `{ group_id, message }` pair. Shared by `sync_pull` and `sync_push`.
fn sync_failures_dto(failed: &[mmcp_sync::GroupSyncFailure]) -> Vec<mmcp_store::Finding> {
    failed
        .iter()
        .map(|f| mmcp_store::Finding {
            group: f.group_id.to_string(),
            slug: None,
            severity: "error",
            code: sync_error_code(&f.error),
            message: f.error.to_string(),
        })
        .collect()
}

#[derive(Debug, Serialize)]
pub struct PullReportDto {
    pub updated: usize,
    pub new_groups: usize,
    /// Groups whose own attempt errored; see `mmcp_sync::GroupSyncFailure`.
    pub failed: Vec<mmcp_store::Finding>,
}

#[derive(Debug, Serialize)]
pub struct PushReportDto {
    pub pushed: usize,
    /// Groups whose own attempt errored; see `mmcp_sync::GroupSyncFailure`.
    pub failed: Vec<mmcp_store::Finding>,
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
pub async fn sync_pull(app: AppHandle, state: State<'_, AppState>) -> GuiResult<PullReportDto> {
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
        .map_err(GuiError::from)?;
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
        failed: sync_failures_dto(&report.failed),
    })
}

#[tauri::command]
pub async fn sync_push(state: State<'_, AppState>) -> GuiResult<PushReportDto> {
    let guard = state.sync.read().await;
    let bundle = guard.as_ref().ok_or(GuiError::SyncNotConfigured)?;
    let report = bundle
        .engine
        .push(SyncFilter::All, &bundle.resolver, &bundle.resolver)
        .await
        .map_err(GuiError::from)?;
    Ok(PushReportDto {
        pushed: report.pushed.len(),
        failed: sync_failures_dto(&report.failed),
    })
}

#[cfg(test)]
mod tests {
    use mmcp_core::id::GroupId;
    use mmcp_sync::{GroupSyncFailure, SyncError};
    use uuid::Uuid;

    use super::*;

    /// Constructs a report with a non-empty `failed` list.
    /// Confirms the resulting `Finding` carries the group id, a stable
    /// code naming the failure cause, and the underlying error
    /// message, not an empty or absent list.
    #[test]
    fn sync_failures_dto_carries_group_id_and_message() {
        let group_id = Uuid::from_u128(0x1234_5678_9abc_def0_1234_5678_9abc_def0);
        let failed = vec![GroupSyncFailure {
            group_id: GroupId::from_uuid(group_id),
            error: SyncError::NotFound("edit-x".into()),
        }];

        let dto = sync_failures_dto(&failed);

        assert_eq!(dto.len(), 1);
        assert_eq!(dto[0].group, group_id.to_string());
        assert_eq!(dto[0].slug, None);
        assert_eq!(dto[0].code, "sync_not_found");
        assert_eq!(dto[0].message, "pending edit not found: edit-x");
    }

    #[test]
    fn empty_failed_list_produces_empty_dto() {
        let dto = sync_failures_dto(&[]);
        assert!(dto.is_empty());
    }
}
