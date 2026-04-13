//! `GitBackend` implementation using local bare repositories via `gix`.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use bytes::Bytes;

use crate::backend::GitBackend;
use crate::error::GitError;
use crate::native::repo_ops;
use crate::types::{CommitMeta, CommitSpec, GroupRef, PushReport, RefSpec, RepoHandle, Rev};

/// Native backend serving bare repositories from a root directory on
/// the local filesystem.
///
/// The on-disk layout is `<root>/<group_uuid>.git`. Each call to
/// [`NativeBackend::create_group_repo`] creates a fresh bare
/// repository; subsequent calls open the existing one.
#[derive(Debug, Clone)]
pub struct NativeBackend {
    root: PathBuf,
}

impl NativeBackend {
    /// Create a new native backend rooted at `root`. The directory is
    /// created if it does not exist.
    pub fn new(root: impl Into<PathBuf>) -> Result<Self, GitError> {
        let root = root.into();
        std::fs::create_dir_all(&root)?;
        Ok(Self { root })
    }

    /// Filesystem path for a group's bare repository.
    fn repo_path(&self, group_id: uuid::Uuid) -> PathBuf {
        self.root.join(format!("{group_id}.git"))
    }

    /// Resolve a repo handle into a filesystem path.
    fn handle_path(handle: &RepoHandle) -> &Path {
        Path::new(&handle.locator)
    }
}

#[async_trait]
impl GitBackend for NativeBackend {
    async fn create_group_repo(&self, group: &GroupRef) -> Result<RepoHandle, GitError> {
        let path = self.repo_path(group.group_id);
        let path_clone = path.clone();
        tokio::task::spawn_blocking(move || repo_ops::init_bare(&path_clone))
            .await
            .map_err(|e| GitError::Gix(format!("join error: {e}")))??;
        Ok(RepoHandle::new(
            group.group_id,
            path.to_string_lossy().into_owned(),
        ))
    }

    async fn clone_to(
        &self,
        _repo: &RepoHandle,
        _dst: &Path,
    ) -> Result<(), GitError> {
        Err(GitError::Unsupported(
            "native backend clone_to requires smart HTTP support",
        ))
    }

    async fn fetch(
        &self,
        _repo: &RepoHandle,
        _refs: &[RefSpec],
    ) -> Result<(), GitError> {
        Err(GitError::Unsupported(
            "native backend fetch requires a remote endpoint",
        ))
    }

    async fn push(
        &self,
        _repo: &RepoHandle,
        _refs: &[RefSpec],
    ) -> Result<PushReport, GitError> {
        Err(GitError::Unsupported(
            "native backend push requires a remote endpoint",
        ))
    }

    async fn read_file(
        &self,
        repo: &RepoHandle,
        path: &str,
        rev: &Rev,
    ) -> Result<Bytes, GitError> {
        let repo_path = Self::handle_path(repo).to_path_buf();
        let path_owned = path.to_string();
        let rev = rev.clone();
        tokio::task::spawn_blocking(move || repo_ops::read_file(&repo_path, &path_owned, &rev))
            .await
            .map_err(|e| GitError::Gix(format!("join error: {e}")))?
    }

    async fn write_commit(
        &self,
        repo: &RepoHandle,
        spec: CommitSpec,
    ) -> Result<String, GitError> {
        let repo_path = Self::handle_path(repo).to_path_buf();
        tokio::task::spawn_blocking(move || repo_ops::write_commit(&repo_path, spec))
            .await
            .map_err(|e| GitError::Gix(format!("join error: {e}")))?
    }

    async fn tag(
        &self,
        repo: &RepoHandle,
        name: &str,
        target: &str,
    ) -> Result<(), GitError> {
        let repo_path = Self::handle_path(repo).to_path_buf();
        let name = name.to_string();
        let target = target.to_string();
        tokio::task::spawn_blocking(move || repo_ops::tag(&repo_path, &name, &target))
            .await
            .map_err(|e| GitError::Gix(format!("join error: {e}")))?
    }

    async fn walk_history(
        &self,
        repo: &RepoHandle,
        path: &str,
    ) -> Result<Vec<CommitMeta>, GitError> {
        let repo_path = Self::handle_path(repo).to_path_buf();
        let path = path.to_string();
        tokio::task::spawn_blocking(move || repo_ops::walk_history(&repo_path, &path))
            .await
            .map_err(|e| GitError::Gix(format!("join error: {e}")))?
    }
}
