//! `GitBackend` implementation using local bare repositories via `gix`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use async_trait::async_trait;
use bytes::Bytes;
use mmcp_core::manifest::GroupManifest;

use crate::backend::GitBackend;
use crate::error::GitError;
use crate::native::repo_ops;
use crate::types::{
    CommitMeta, CommitSpec, Credentials, FastForwardOutcome, PushReport, RefSpec, RepoHandle, Rev,
};

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

    /// Already-opened repository handles, keyed by their bare-repo
    /// path and shared across every clone of this backend.
    ///
    /// Opening a bare repository through `gix` runs discovery, parses
    /// the full git config, and mounts the object database; every
    /// single-primitive read (`list_tree`, `read_file`, ...) used to
    /// pay that cost on every call. A `gix::ThreadSafeRepository` is
    /// cheap to clone and safe to hold for the process lifetime, so
    /// the first open per path is cached here and reused; each
    /// caller derives its own thread-local `gix::Repository` from it
    /// with `to_thread_local()` inside its own `spawn_blocking`.
    repo_cache: Arc<Mutex<HashMap<PathBuf, gix::ThreadSafeRepository>>>,
}

impl NativeBackend {
    /// Create a new native backend rooted at `root`. The directory is
    /// created if it does not exist. No startup probe of the `git`
    /// binary runs here: read/write/tag/history/tree operations go
    /// through `gix` in-process and never need it. The remaining
    /// `clone_to` / `fetch` / `push` paths that still shell out to
    /// `git` surface their own error lazily when invoked, which is
    /// good enough for server builds that never reach those paths.
    pub fn new(root: impl Into<PathBuf>) -> Result<Self, GitError> {
        let root = root.into();
        std::fs::create_dir_all(&root)?;
        Ok(Self {
            root,
            repo_cache: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    /// Filesystem path for a group's bare repository.
    ///
    /// Exposed so callers outside this crate can probe existence
    /// before invoking [`GitBackend::create_group_repo`]: the
    /// `<repos_root>/<uuid>.git` layout is already a de-facto public
    /// contract that the client's `GroupIndex` relies on.
    pub fn repo_path(&self, group_id: uuid::Uuid) -> PathBuf {
        self.root.join(format!("{group_id}.git"))
    }

    /// Resolve a repo handle into a filesystem path.
    fn handle_path(handle: &RepoHandle) -> &Path {
        Path::new(&handle.locator)
    }

    /// Open (or reuse a cached handle for) the bare repository at
    /// `path`.
    ///
    /// The first call per path pays the full `gix` discovery,
    /// config-parse, and ODB-mount cost and caches the resulting
    /// handle; every later call for the same path is a map lookup, a
    /// cheap `path.exists()` stat, and a clone. Runs synchronously:
    /// callers invoke this from inside their own `spawn_blocking`
    /// closure, the same as every other `gix` call in this backend.
    ///
    /// The `exists()` stat runs even on a cache hit: a cached
    /// `gix::ThreadSafeRepository` keeps working against a bare repo
    /// deleted out from under it (loose refs and objects already
    /// resolved once can keep resolving from in-memory state), which
    /// would silently mask the loss instead of surfacing it. A
    /// vanished path evicts the stale entry and reports
    /// [`GitError::RepoNotFound`] instead of serving that stale
    /// state.
    fn open_repo(&self, path: &Path) -> Result<gix::ThreadSafeRepository, GitError> {
        let mut cache = self
            .repo_cache
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(repo) = cache.get(path) {
            if path.exists() {
                return Ok(repo.clone());
            }
            cache.remove(path);
            return Err(GitError::RepoNotFound(path.to_string_lossy().into_owned()));
        }
        if !path.exists() {
            return Err(GitError::RepoNotFound(path.to_string_lossy().into_owned()));
        }
        let repo = gix::ThreadSafeRepository::open(path).map_err(|e| GitError::OpenRepo {
            path: path.to_string_lossy().into_owned(),
            source: Box::new(e),
        })?;
        cache.insert(path.to_path_buf(), repo.clone());
        Ok(repo)
    }

    /// Read the contents of every path in `paths` at the same
    /// revision, resolving the commit and its root tree once instead
    /// of once per file. Each path keeps its own outcome, in request
    /// order, so one missing or unreadable file never aborts the
    /// batch. Not part of [`GitBackend`]: batching is an
    /// optimization primitive for callers that already know they
    /// need several files at once, not a capability every backend
    /// must implement.
    pub async fn read_files(
        &self,
        repo: &RepoHandle,
        paths: Vec<String>,
        rev: &Rev,
    ) -> Result<repo_ops::BatchReadResult, GitError> {
        let repo_path = Self::handle_path(repo).to_path_buf();
        let rev = rev.clone();
        let backend = self.clone();
        tokio::task::spawn_blocking(move || {
            let handle = backend.open_repo(&repo_path)?;
            repo_ops::read_files(&handle.to_thread_local(), &paths, &rev)
        })
        .await?
    }

    /// Resolve `rev` to its tip commit and return that commit's
    /// metadata, without walking history. Not part of [`GitBackend`]
    /// for the same reason as [`Self::read_files`]: it is a
    /// tip-only-read optimization over [`GitBackend::walk_history`],
    /// not a capability every backend must implement.
    pub async fn tip_commit(&self, repo: &RepoHandle, rev: &Rev) -> Result<CommitMeta, GitError> {
        let repo_path = Self::handle_path(repo).to_path_buf();
        let rev = rev.clone();
        let backend = self.clone();
        tokio::task::spawn_blocking(move || {
            let handle = backend.open_repo(&repo_path)?;
            repo_ops::tip_commit(&handle.to_thread_local(), &rev)
        })
        .await?
    }
}

#[async_trait]
impl GitBackend for NativeBackend {
    async fn create_group_repo(&self, manifest: &GroupManifest) -> Result<RepoHandle, GitError> {
        let uuid = *manifest.group_id.as_uuid();
        let path = self.repo_path(uuid);
        let path_clone = path.clone();
        tokio::task::spawn_blocking(move || repo_ops::init_bare(&path_clone)).await??;
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

    async fn read_manifest(&self, repo: &RepoHandle) -> Result<GroupManifest, GitError> {
        let bytes = self
            .read_file(repo, mmcp_core::manifest::MANIFEST_FILENAME, &Rev::head())
            .await?;
        let text = std::str::from_utf8(&bytes).map_err(|e| GitError::Manifest {
            operation: "decode",
            source: Box::new(e),
        })?;
        GroupManifest::from_toml(text).map_err(|e| GitError::Manifest {
            operation: "decode",
            source: Box::new(e),
        })
    }

    async fn write_manifest(
        &self,
        repo: &RepoHandle,
        manifest: &GroupManifest,
    ) -> Result<String, GitError> {
        let rendered = manifest.to_toml().map_err(|e| GitError::Manifest {
            operation: "encode",
            source: Box::new(e),
        })?;
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
        creds: &Credentials,
    ) -> Result<(), GitError> {
        let dst = dst.to_path_buf();
        let remote_url = remote_url.to_string();
        let creds = creds.clone();
        tokio::task::spawn_blocking(move || repo_ops::clone(&remote_url, &dst, &creds)).await?
    }

    async fn fetch(
        &self,
        repo: &RepoHandle,
        remote_url: &str,
        refs: &[RefSpec],
        creds: &Credentials,
    ) -> Result<(), GitError> {
        let repo_path = Self::handle_path(repo).to_path_buf();
        let remote_url = remote_url.to_string();
        // Format refspecs the way `git fetch` expects, including the
        // `+` prefix for forced updates. Without the prefix git refuses
        // to overwrite a diverged local ref, so callers who opted into
        // `RefSpec::forced()` would still hit non-fast-forward
        // rejections.
        let refspecs: Vec<String> = refs
            .iter()
            .map(|r| {
                let prefix = if r.force { "+" } else { "" };
                format!("{prefix}{}:{}", r.local, r.remote)
            })
            .collect();
        let creds = creds.clone();
        tokio::task::spawn_blocking(move || {
            repo_ops::fetch(&repo_path, &remote_url, &refspecs, &creds)
        })
        .await?
    }

    async fn push(
        &self,
        repo: &RepoHandle,
        remote_url: &str,
        refs: &[RefSpec],
        creds: &Credentials,
    ) -> Result<PushReport, GitError> {
        let repo_path = Self::handle_path(repo).to_path_buf();
        let remote_url = remote_url.to_string();
        let refspecs: Vec<(String, String, bool)> = refs
            .iter()
            .map(|r| (r.local.clone(), r.remote.clone(), r.force))
            .collect();
        let creds = creds.clone();
        let backend = self.clone();
        tokio::task::spawn_blocking(move || {
            let handle = backend.open_repo(&repo_path)?;
            repo_ops::push(&handle.to_thread_local(), &remote_url, &refspecs, &creds)
        })
        .await?
    }

    async fn read_file(&self, repo: &RepoHandle, path: &str, rev: &Rev) -> Result<Bytes, GitError> {
        let repo_path = Self::handle_path(repo).to_path_buf();
        let path_owned = path.to_string();
        let rev = rev.clone();
        let backend = self.clone();
        tokio::task::spawn_blocking(move || {
            let handle = backend.open_repo(&repo_path)?;
            repo_ops::read_file(&handle.to_thread_local(), &path_owned, &rev)
        })
        .await?
    }

    async fn write_commit(&self, repo: &RepoHandle, spec: CommitSpec) -> Result<String, GitError> {
        let repo_path = Self::handle_path(repo).to_path_buf();
        let backend = self.clone();
        tokio::task::spawn_blocking(move || {
            let handle = backend.open_repo(&repo_path)?;
            repo_ops::write_commit(&handle.to_thread_local(), spec)
        })
        .await?
    }

    async fn tag(&self, repo: &RepoHandle, name: &str, target: &str) -> Result<(), GitError> {
        let repo_path = Self::handle_path(repo).to_path_buf();
        let name = name.to_string();
        let target = target.to_string();
        let backend = self.clone();
        tokio::task::spawn_blocking(move || {
            let handle = backend.open_repo(&repo_path)?;
            repo_ops::tag(&handle.to_thread_local(), &name, &target)
        })
        .await?
    }

    async fn fast_forward(
        &self,
        repo: &RepoHandle,
        local_ref: &str,
        target_ref: &str,
    ) -> Result<FastForwardOutcome, GitError> {
        let repo_path = Self::handle_path(repo).to_path_buf();
        let local_ref = local_ref.to_string();
        let target_ref = target_ref.to_string();
        let backend = self.clone();
        tokio::task::spawn_blocking(move || {
            let handle = backend.open_repo(&repo_path)?;
            repo_ops::fast_forward(&handle.to_thread_local(), &local_ref, &target_ref)
        })
        .await?
    }

    async fn walk_history(
        &self,
        repo: &RepoHandle,
        path: &str,
    ) -> Result<Vec<CommitMeta>, GitError> {
        let repo_path = Self::handle_path(repo).to_path_buf();
        let path = path.to_string();
        let backend = self.clone();
        tokio::task::spawn_blocking(move || {
            let handle = backend.open_repo(&repo_path)?;
            repo_ops::walk_history(&handle.to_thread_local(), &path)
        })
        .await?
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
        let backend = self.clone();
        tokio::task::spawn_blocking(move || {
            let handle = backend.open_repo(&repo_path)?;
            repo_ops::list_tree(&handle.to_thread_local(), &prefix, &rev)
        })
        .await?
    }

    async fn list_subtrees(
        &self,
        repo: &RepoHandle,
        path_prefix: &str,
        rev: &Rev,
    ) -> Result<Vec<String>, GitError> {
        let repo_path = Self::handle_path(repo).to_path_buf();
        let prefix = path_prefix.to_string();
        let rev = rev.clone();
        let backend = self.clone();
        tokio::task::spawn_blocking(move || {
            let handle = backend.open_repo(&repo_path)?;
            repo_ops::list_subtrees(&handle.to_thread_local(), &prefix, &rev)
        })
        .await?
    }
}
