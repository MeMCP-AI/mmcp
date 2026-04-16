//! `GitBackend` implementation using local bare repositories via `gix`.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use bytes::Bytes;
use mmcp_core::manifest::GroupManifest;

use crate::backend::GitBackend;
use crate::error::GitError;
use crate::native::repo_ops;
use crate::types::{CommitMeta, CommitSpec, PushReport, RefSpec, RepoHandle, Rev};

/// Native backend serving bare repositories from a root directory on
/// the local filesystem.
///
/// The on-disk layout is `<root>/<group_uuid>.git`. Each call to
/// [`NativeBackend::create_group_repo`] initialises a bare repo if
/// needed and commits the initial `.mmcp.toml` manifest on `main`
/// if the repo does not already have one.
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
    async fn create_group_repo(
        &self,
        manifest: &GroupManifest,
    ) -> Result<RepoHandle, GitError> {
        let uuid = *manifest.group_id.as_uuid();
        let path = self.repo_path(uuid);
        let path_clone = path.clone();
        tokio::task::spawn_blocking(move || repo_ops::init_bare(&path_clone))
            .await
            .map_err(|e| GitError::Gix(format!("join error: {e}")))??;
        let handle = RepoHandle::new(uuid, path.to_string_lossy().into_owned());

        // Only write an initial manifest if the repo does not
        // already carry one. This keeps `create_group_repo`
        // idempotent against re-runs with the same manifest.
        let already_has_manifest = self.read_manifest(&handle).await.is_ok();
        if !already_has_manifest {
            self.write_manifest(&handle, manifest).await?;
        }
        Ok(handle)
    }

    async fn read_manifest(
        &self,
        repo: &RepoHandle,
    ) -> Result<GroupManifest, GitError> {
        let bytes = self
            .read_file(
                repo,
                mmcp_core::manifest::MANIFEST_FILENAME,
                &Rev::head(),
            )
            .await?;
        let text = std::str::from_utf8(&bytes)
            .map_err(|e| GitError::Gix(format!("manifest is not valid UTF-8: {e}")))?;
        GroupManifest::from_toml(text).map_err(|e| GitError::Gix(format!("manifest parse: {e}")))
    }

    async fn write_manifest(
        &self,
        repo: &RepoHandle,
        manifest: &GroupManifest,
    ) -> Result<String, GitError> {
        let rendered = manifest
            .to_toml()
            .map_err(|e| GitError::Gix(format!("manifest render: {e}")))?;
        self.write_commit(
            repo,
            CommitSpec::mmcp_commit(
                "mmcp: initialize group manifest",
                vec![(
                    mmcp_core::manifest::MANIFEST_FILENAME.to_string(),
                    Some(rendered.into_bytes()),
                )],
                mmcp_core::conventions::MMCP_AUTHOR_NAME,
                mmcp_core::conventions::MMCP_AUTHOR_EMAIL,
            ),
        )
        .await
    }

    async fn clone_to(
        &self,
        remote_url: &str,
        dst: &Path,
    ) -> Result<(), GitError> {
        let dst = dst.to_path_buf();
        let remote_url = remote_url.to_string();
        tokio::task::spawn_blocking(move || repo_ops::clone(&remote_url, &dst))
            .await
            .map_err(|e| GitError::Gix(format!("join error: {e}")))?
    }

    async fn fetch(
        &self,
        repo: &RepoHandle,
        remote_url: &str,
        refs: &[RefSpec],
    ) -> Result<(), GitError> {
        let repo_path = Self::handle_path(repo).to_path_buf();
        let remote_url = remote_url.to_string();
        let refspecs: Vec<String> = refs
            .iter()
            .map(|r| format!("{}:{}", r.local, r.remote))
            .collect();
        tokio::task::spawn_blocking(move || repo_ops::fetch(&repo_path, &remote_url, &refspecs))
            .await
            .map_err(|e| GitError::Gix(format!("join error: {e}")))?
    }

    async fn push(
        &self,
        repo: &RepoHandle,
        remote_url: &str,
        refs: &[RefSpec],
    ) -> Result<PushReport, GitError> {
        let repo_path = Self::handle_path(repo).to_path_buf();
        let remote_url = remote_url.to_string();
        let refspecs: Vec<(String, String, bool)> = refs
            .iter()
            .map(|r| (r.local.clone(), r.remote.clone(), r.force))
            .collect();
        tokio::task::spawn_blocking(move || repo_ops::push(&repo_path, &remote_url, &refspecs))
            .await
            .map_err(|e| GitError::Gix(format!("join error: {e}")))?
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

    async fn list_tree(
        &self,
        repo: &RepoHandle,
        path_prefix: &str,
        rev: &Rev,
    ) -> Result<Vec<String>, GitError> {
        let repo_path = Self::handle_path(repo).to_path_buf();
        let prefix = path_prefix.to_string();
        let rev = rev.clone();
        tokio::task::spawn_blocking(move || repo_ops::list_tree(&repo_path, &prefix, &rev))
            .await
            .map_err(|e| GitError::Gix(format!("join error: {e}")))?
    }
}
