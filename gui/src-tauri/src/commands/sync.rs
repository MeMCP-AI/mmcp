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

/// One group whose sync attempt errored.
/// Wire mirror of `mmcp_sync::GroupSyncFailure`, whose `SyncError` field is not `Serialize`.
#[derive(Debug, Serialize)]
pub struct GroupSyncFailureDto {
    pub group_id: String,
    pub message: String,
}

/// Convert an engine report's `failed` list into its DTO shape.
/// Shared by `sync_pull` and `sync_push`.
fn sync_failures_dto(failed: &[mmcp_sync::GroupSyncFailure]) -> Vec<GroupSyncFailureDto> {
    failed
        .iter()
        .map(|f| GroupSyncFailureDto {
            group_id: f.group_id.to_string(),
            message: f.error.to_string(),
        })
        .collect()
}

#[derive(Debug, Serialize)]
pub struct PullReportDto {
    pub updated: usize,
    pub new_groups: usize,
    /// Groups whose own attempt errored; see `mmcp_sync::GroupSyncFailure`.
    pub failed: Vec<GroupSyncFailureDto>,
}

#[derive(Debug, Serialize)]
pub struct PushReportDto {
    pub pushed: usize,
    /// Groups whose own attempt errored; see `mmcp_sync::GroupSyncFailure`.
    pub failed: Vec<GroupSyncFailureDto>,
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
    use mmcp_sync::{GroupSyncFailure, SyncError};
    use uuid::Uuid;

    use super::*;

    /// FALSIFICATION: before this fix, `sync_failures_dto` did not
    /// exist and `PullReportDto`/`PushReportDto` had no `failed`
    /// field at all, so a per-group failure was silently dropped
    /// (the DTO carried zero information about it). This test
    /// constructs a report with a non-empty `failed` list and
    /// confirms the resulting DTO actually carries the group id and
    /// the underlying error message, not an empty/absent list.
    #[test]
    fn sync_failures_dto_carries_group_id_and_message() {
        let group_id = Uuid::from_u128(0x1234_5678_9abc_def0_1234_5678_9abc_def0);
        let failed = vec![GroupSyncFailure {
            group_id,
            error: SyncError::NotFound("edit-x".into()),
        }];

        let dto = sync_failures_dto(&failed);

        assert_eq!(dto.len(), 1);
        assert_eq!(dto[0].group_id, group_id.to_string());
        assert_eq!(dto[0].message, "pending edit not found: edit-x");
    }

    #[test]
    fn empty_failed_list_produces_empty_dto() {
        let dto = sync_failures_dto(&[]);
        assert!(dto.is_empty());
    }
}
