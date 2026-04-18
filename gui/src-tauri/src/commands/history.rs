//! Memory commit-history commands. Backs the history-visualisation
//! panel in the viewer: list every commit that touched the resolved
//! memory path, then read the file back at an arbitrary commit so
//! the UI can show the memory as it was at that point in time.

use mmcp_core::id::GroupId;
use mmcp_core::memory::MemoryFile;
use mmcp_git::{GitBackend, Rev};
use mmcp_store::resolve_memory;
use serde::Serialize;
use tauri::State;
use uuid::Uuid;

use crate::commands::memory::MemoryFileDto;
use crate::error::{GuiError, GuiResult};
use crate::state::AppState;

#[derive(Debug, Serialize)]
pub struct CommitMetaDto {
    pub id: String,
    pub short_id: String,
    pub subject: String,
    pub message: String,
    pub author_name: String,
    pub author_email: String,
    /// Seconds since the Unix epoch. The frontend renders relative
    /// and absolute times from this — no need for a second call.
    pub timestamp: i64,
}

fn group_id_from_str(s: &str) -> GuiResult<GroupId> {
    let uuid = Uuid::parse_str(s).map_err(|e| GuiError::Other(format!("group_id uuid: {e}")))?;
    Ok(GroupId::from_uuid(uuid))
}

#[tauri::command]
pub async fn list_memory_history(
    group_id: String,
    slug: String,
    state: State<'_, AppState>,
) -> GuiResult<Vec<CommitMetaDto>> {
    let gid = group_id_from_str(&group_id)?;
    let entry = state
        .index
        .get(&gid)
        .await
        .ok_or_else(|| GuiError::Other(format!("group {group_id} is not in the local mirror")))?;
    let resolved = resolve_memory(&state.backend, &entry.handle, Some(&slug), None)
        .await
        .map_err(GuiError::from)?;
    let commits = state
        .backend
        .walk_history(&entry.handle, &resolved.path)
        .await
        .map_err(GuiError::from)?;
    Ok(commits
        .into_iter()
        .map(|c| {
            // 7-char prefix matches git's default short-hash width;
            // long enough to stay unambiguous in practice.
            let short = c.id.chars().take(7).collect();
            CommitMetaDto {
                id: c.id,
                short_id: short,
                subject: c.subject,
                message: c.message,
                author_name: c.author_name,
                author_email: c.author_email,
                timestamp: c.timestamp,
            }
        })
        .collect())
}

#[tauri::command]
pub async fn load_memory_at(
    group_id: String,
    slug: String,
    commit: String,
    state: State<'_, AppState>,
) -> GuiResult<MemoryFileDto> {
    let gid = group_id_from_str(&group_id)?;
    let entry = state
        .index
        .get(&gid)
        .await
        .ok_or_else(|| GuiError::Other(format!("group {group_id} is not in the local mirror")))?;
    let resolved = resolve_memory(&state.backend, &entry.handle, Some(&slug), None)
        .await
        .map_err(GuiError::from)?;
    let bytes = state
        .backend
        .read_file(&entry.handle, &resolved.path, &Rev::Commit(commit))
        .await
        .map_err(GuiError::from)?;
    let text = std::str::from_utf8(&bytes)?;
    let mf = MemoryFile::parse(text).map_err(GuiError::from)?;
    Ok(MemoryFileDto::from(&mf))
}
