//! Group listing / refresh commands.

use serde::Serialize;
use tauri::State;

use crate::error::{GuiError, GuiResult};
use crate::state::AppState;

#[derive(Debug, Serialize)]
pub struct GroupEntryDto {
    pub group_id: String,
    pub slug: String,
    pub display_name: Option<String>,
    pub memory_count_hint: u32,
    /// Manifest scope, serialised as `global` / `shared` / `project`.
    /// The repo-style variant uses this to bucket groups into the
    /// three top-level "repo" tabs the user sees in that layout.
    pub scope: String,
}

impl From<&mmcp_store::GroupEntry> for GroupEntryDto {
    fn from(entry: &mmcp_store::GroupEntry) -> Self {
        GroupEntryDto {
            group_id: entry.manifest.group_id.to_string(),
            slug: entry.manifest.slug.clone(),
            display_name: entry.manifest.display_name.clone(),
            memory_count_hint: 0,
            scope: match entry.manifest.scope {
                mmcp_core::manifest::GroupScope::Global => "global".into(),
                mmcp_core::manifest::GroupScope::Shared => "shared".into(),
                mmcp_core::manifest::GroupScope::Project => "project".into(),
            },
        }
    }
}

#[tauri::command]
pub async fn list_groups(state: State<'_, AppState>) -> GuiResult<Vec<GroupEntryDto>> {
    let out: Vec<GroupEntryDto> = state.index.list().await.iter().map(Into::into).collect();
    tracing::debug!(count = out.len(), "ipc: list_groups");
    Ok(out)
}

#[tauri::command]
pub async fn refresh_groups(state: State<'_, AppState>) -> GuiResult<Vec<GroupEntryDto>> {
    state.index.refresh().await.map_err(GuiError::from)?;
    Ok(state.index.list().await.iter().map(Into::into).collect())
}
