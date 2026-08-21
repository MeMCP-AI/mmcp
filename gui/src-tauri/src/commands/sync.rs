//! Sync commands (pull / push / status snapshot).

use mmcp_sync::{PushScope, SyncFilter};
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
    /// Human-readable label for the effective remote set; see
    /// [`crate::state::SyncBundle::remotes_summary`]. A bundle can
    /// carry several remotes, so this summarises the whole effective
    /// set rather than naming a single server.
    pub remotes_summary: Option<String>,
    /// Reason sync failed to resolve at startup or the last rebuild,
    /// e.g. an ambiguous default remote across the merged config.
    /// See [`crate::state::AppState::sync_error`].
    ///
    /// `None` when `configured` is true, OR when `configured` is
    /// false because no remotes are declared anywhere, which is not
    /// an error.
    pub error: Option<String>,
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
        mmcp_sync::SyncError::NoDefaultRemote => "sync_no_default_remote",
        mmcp_sync::SyncError::UnknownRemote { .. } => "sync_unknown_remote",
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

/// One `mmcp-server`-transport remote whose manifest poll itself
/// errored during a pull's `fetch` phase; see
/// `mmcp_sync::RemoteManifestFailure`. A remote-level failure, not a
/// group-level one, so it carries no group id and is kept as its own
/// DTO rather than overloading [`mmcp_store::Finding::group`] with a
/// remote name.
#[derive(Debug, Serialize)]
pub struct RemoteManifestFailureDto {
    pub remote_name: String,
    pub code: &'static str,
    pub message: String,
}

fn manifest_failures_dto(
    failed: &[mmcp_sync::RemoteManifestFailure],
) -> Vec<RemoteManifestFailureDto> {
    failed
        .iter()
        .map(|f| RemoteManifestFailureDto {
            remote_name: f.remote_name.clone(),
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
    /// Remotes whose manifest poll itself errored; see
    /// `mmcp_sync::PullReport::manifest_failures`. Surfaced rather
    /// than dropped, per the no-silent-failure rule: an unreachable
    /// remote never disappears from the report.
    pub manifest_failures: Vec<RemoteManifestFailureDto>,
}

/// One remote's push outcome, mirroring `mmcp_sync::RemotePushOutcome`
/// with the same summarised failure shape `sync_pull` uses. Kept
/// per-remote rather than flattened: `PushScope::All` (not exposed by
/// the GUI yet, but `mmcp_sync::PushScope` already supports it) can
/// target more than one remote, and flattening would lose which
/// remote a given failure belongs to.
#[derive(Debug, Serialize)]
pub struct RemotePushOutcomeDto {
    pub remote_name: String,
    pub pushed: usize,
    /// Groups whose own attempt errored; see `mmcp_sync::GroupSyncFailure`.
    pub failed: Vec<mmcp_store::Finding>,
}

#[derive(Debug, Serialize)]
pub struct PushReportDto {
    pub by_remote: Vec<RemotePushOutcomeDto>,
}

#[tauri::command]
pub async fn sync_status(state: State<'_, AppState>) -> GuiResult<SyncStatusDto> {
    let guard = state.sync.read().await;
    let error = state.sync_error.read().await.clone();
    Ok(SyncStatusDto {
        configured: guard.is_some(),
        remotes_summary: guard.as_ref().map(|s| s.remotes_summary.clone()),
        error,
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
        manifest_failures: manifest_failures_dto(&report.manifest_failures),
    })
}

#[tauri::command]
pub async fn sync_push(state: State<'_, AppState>) -> GuiResult<PushReportDto> {
    let guard = state.sync.read().await;
    let bundle = guard.as_ref().ok_or(GuiError::SyncNotConfigured)?;
    // `PushScope::Default` matches the CLI's own default landing
    // point (`mmcp push` with no `--all-remotes`/`--remote` flag);
    // the GUI doesn't expose a remote-scope picker yet.
    let report = bundle
        .engine
        .push(
            SyncFilter::All,
            PushScope::Default,
            &bundle.resolver,
            &bundle.resolver,
        )
        .await
        .map_err(GuiError::from)?;
    Ok(PushReportDto {
        by_remote: report
            .by_remote
            .iter()
            .map(|r| RemotePushOutcomeDto {
                remote_name: r.remote_name.clone(),
                pushed: r.pushed.len(),
                failed: sync_failures_dto(&r.failed),
            })
            .collect(),
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

    /// `sync_error_code`'s match must stay exhaustive over every
    /// `SyncError` variant `mmcp_sync::PushScope` resolution can
    /// raise: `NoDefaultRemote` (an empty or ambiguous effective set)
    /// and `UnknownRemote` (a `PushScope::Named` selector with no
    /// matching bound remote) each get their own stable code, never
    /// falling through to a shared or absent arm.
    #[test]
    fn no_default_remote_and_unknown_remote_map_to_their_own_stable_codes() {
        assert_eq!(
            sync_error_code(&SyncError::NoDefaultRemote),
            "sync_no_default_remote"
        );
        assert_eq!(
            sync_error_code(&SyncError::UnknownRemote {
                name: "mirror".to_string()
            }),
            "sync_unknown_remote"
        );
    }

    /// A remote-level manifest failure carries the failing remote's
    /// name and a stable code, distinct from `sync_failures_dto`'s
    /// group-keyed `Finding`s: there is no group id to attach a
    /// manifest poll failure to.
    #[test]
    fn manifest_failures_dto_carries_remote_name_and_message() {
        let failed = vec![mmcp_sync::RemoteManifestFailure {
            remote_name: "mirror".to_string(),
            error: SyncError::Transport("connection refused".to_string()),
        }];

        let dto = manifest_failures_dto(&failed);

        assert_eq!(dto.len(), 1);
        assert_eq!(dto[0].remote_name, "mirror");
        assert_eq!(dto[0].code, "sync_transport_error");
        assert_eq!(dto[0].message, "transport error: connection refused");
    }

    #[test]
    fn empty_manifest_failures_produces_empty_dto() {
        assert!(manifest_failures_dto(&[]).is_empty());
    }
}
