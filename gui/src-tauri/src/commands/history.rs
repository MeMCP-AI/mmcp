//! Memory commit-history commands. Backs the history-visualisation
//! panel in the viewer: list every commit that touched the resolved
//! memory path, then read the file back at an arbitrary commit so
//! the UI can show the memory as it was at that point in time.

use mmcp_core::id::GroupId;
use mmcp_core::memory::MemoryFile;
use mmcp_git::{GitBackend, Rev};
use mmcp_store::resolve_memory;
use serde::Serialize;
use similar::{ChangeTag, TextDiff};
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

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiffLineKind {
    Equal,
    Insert,
    Delete,
}

#[derive(Debug, Serialize)]
pub struct DiffLine {
    pub kind: DiffLineKind,
    pub old_lineno: Option<u32>,
    pub new_lineno: Option<u32>,
    pub text: String,
}

#[derive(Debug, Serialize)]
pub struct DiffResult {
    /// `None` when the file did not exist at the base (pure insert);
    /// `Some(hex)` otherwise.
    pub from: Option<String>,
    pub to: String,
    /// Empty when the two sides are byte-identical.
    pub lines: Vec<DiffLine>,
    pub inserted: u32,
    pub deleted: u32,
}

async fn read_file_at_commit(
    state: &AppState,
    group_id: &str,
    slug: &str,
    commit: &str,
) -> GuiResult<String> {
    let gid = group_id_from_str(group_id)?;
    let entry = state
        .index
        .get(&gid)
        .await
        .ok_or_else(|| GuiError::Other(format!("group {group_id} is not in the local mirror")))?;
    let resolved = resolve_memory(&state.backend, &entry.handle, Some(slug), None)
        .await
        .map_err(GuiError::from)?;
    let bytes = state
        .backend
        .read_file(&entry.handle, &resolved.path, &Rev::Commit(commit.to_string()))
        .await
        .map_err(GuiError::from)?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// Produce a line diff of the memory between two commits. Pass
/// `from = None` to diff against "nothing" (the commit that first
/// introduced the file); the result is then an all-insert diff
/// and `from` echoes back as `None` so the UI can label it as
/// "introduced in this commit".
#[tauri::command]
pub async fn diff_memory(
    group_id: String,
    slug: String,
    from: Option<String>,
    to: String,
    state: State<'_, AppState>,
) -> GuiResult<DiffResult> {
    let text_to = read_file_at_commit(&state, &group_id, &slug, &to).await?;
    let text_from = match &from {
        Some(hex) => read_file_at_commit(&state, &group_id, &slug, hex).await?,
        None => String::new(),
    };

    let diff = TextDiff::from_lines(&text_from, &text_to);
    let mut lines = Vec::new();
    let mut inserted = 0u32;
    let mut deleted = 0u32;
    for change in diff.iter_all_changes() {
        let (kind, old_idx, new_idx) = match change.tag() {
            ChangeTag::Equal => (
                DiffLineKind::Equal,
                change.old_index(),
                change.new_index(),
            ),
            ChangeTag::Insert => {
                inserted += 1;
                (DiffLineKind::Insert, None, change.new_index())
            }
            ChangeTag::Delete => {
                deleted += 1;
                (DiffLineKind::Delete, change.old_index(), None)
            }
        };
        let raw = change.value();
        // `similar` preserves trailing newlines per change. Strip a
        // single one to keep the wire format compact; a completely
        // empty string is a meaningful blank line and stays so.
        let text = raw.strip_suffix('\n').unwrap_or(raw).to_string();
        lines.push(DiffLine {
            kind,
            old_lineno: old_idx.map(|i| (i + 1) as u32),
            new_lineno: new_idx.map(|i| (i + 1) as u32),
            text,
        });
    }

    Ok(DiffResult {
        from,
        to,
        lines,
        inserted,
        deleted,
    })
}
