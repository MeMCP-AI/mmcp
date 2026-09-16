//! Single owning primitive for walking a repository path's commit history.
//!
//! Every caller that needs raw commit history (the `list_versions` MCP tool,
//! the CLI `mmcp memory versions` command, the `debug_git_log` diagnostic tool)
//! goes through [`walk_path_history`] instead of calling [`GitBackend::walk_history`] directly,
//! so a later cross-cutting change has one call site to update.

use mmcp_git::{CommitMeta, GitBackend, GitError, NativeBackend, RepoHandle};

/// Walk the commit history of `path` in `handle`'s repo, most recent first.
/// `limit` caps the number of entries returned; `None` is unbounded.
pub async fn walk_path_history(
    backend: &NativeBackend,
    handle: &RepoHandle,
    path: &str,
    limit: Option<usize>,
) -> Result<Vec<CommitMeta>, GitError> {
    backend.walk_history(handle, path, limit).await
}
