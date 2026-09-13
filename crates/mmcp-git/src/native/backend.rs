//! `GitBackend` implementation using local bare repositories via `gix`.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use bytes::Bytes;
use mmcp_core::manifest::GroupManifest;
use moka::sync::Cache;

use crate::backend::GitBackend;
use crate::error::GitError;
use crate::native::defaults::{OBJECT_CACHE_SIZE_BYTES, REPO_CACHE_MAX_ENTRIES};
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
    /// via [`Self::thread_local_with_object_cache`] inside its own
    /// `spawn_blocking`.
    ///
    /// Bounded by [`REPO_CACHE_MAX_ENTRIES`] instead of an unbounded
    /// `HashMap`: a long-lived server process touching many distinct
    /// group repositories over its lifetime must not grow this map
    /// without limit. `moka::sync::Cache` is itself cheaply `Clone`
    /// (internally `Arc`-backed) and thread-safe, so no outer
    /// `Arc<Mutex<_>>` wrapper is needed; eviction is safe because
    /// every live caller already holds its own clone of the
    /// `ThreadSafeRepository` it is using, so an evicted entry only
    /// means the next `open_repo` call re-pays discovery cost.
    repo_cache: Cache<PathBuf, gix::ThreadSafeRepository>,
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
            repo_cache: Cache::builder()
                .max_capacity(REPO_CACHE_MAX_ENTRIES)
                .build(),
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
    /// `path`, given its cache directly instead of a whole `&self`.
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
    ///
    /// Taking `cache` directly, rather than `&self`, matters because
    /// every `GitBackend` method below moves its work into a
    /// `spawn_blocking` closure that needs nothing from `NativeBackend`
    /// except this cache: `repo_path` is already computed and moved in
    /// separately, and `self.root` never crosses that boundary. Cloning
    /// the cache alone (`moka::sync::Cache` is `Arc`-backed, so this is
    /// one atomic increment) instead of the whole backend (which also
    /// heap-allocates a fresh `PathBuf` copy of `root` on every call)
    /// avoids that unused allocation per dispatched operation.
    fn open_repo_with_cache(
        cache: &Cache<PathBuf, gix::ThreadSafeRepository>,
        path: &Path,
    ) -> Result<gix::ThreadSafeRepository, GitError> {
        if let Some(repo) = cache.get(path) {
            if path.exists() {
                return Ok(repo);
            }
            cache.invalidate(path);
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

    /// Derive a thread-local [`gix::Repository`] from a cached handle,
    /// with its decoded-object cache enabled.
    ///
    /// `to_thread_local()` produces a fresh `Repository` per call, so
    /// this only pays off within the single `spawn_blocking` closure
    /// that calls it: the same root tree and `memories/` tree get
    /// decoded once per operation instead of once per access inside
    /// that operation (a `read_files` batch, a recursive tree
    /// descent). It does not need to persist beyond that closure, so
    /// there is no invalidation concern beyond the cache's own normal
    /// eviction.
    fn thread_local_with_object_cache(handle: &gix::ThreadSafeRepository) -> gix::Repository {
        let mut repo = handle.to_thread_local();
        repo.object_cache_size_if_unset(OBJECT_CACHE_SIZE_BYTES);
        repo
    }

    /// Evict `path`'s cached repository handle, if any.
    ///
    /// `open_repo_with_cache`'s cache-validity check only catches deletion (its `path.exists()` check).
    /// It cannot detect an in-place replacement where a new bare repository is renamed onto the same path,
    /// because the path still exists throughout the swap.
    /// This is exactly the sequence `mmcp_store::archive::import` uses to restore a bare repository in place.
    /// Callers that replace a repository's contents in place must call this immediately after the swap.
    /// The next `open_repo_with_cache` call re-opens fresh,
    /// instead of relying on the cached `gix::ThreadSafeRepository` to notice the replacement on its own.
    /// Under the pinned gix build and mmcp's loose-object-only write path, this self-heals today.
    /// Verified empirically by `crates/mmcp-git/tests/native_backend.rs`,
    /// test `in_place_repo_swap_on_same_path_is_visible_after_invalidate`.
    /// So this call is a forward guard against a gix caching change (e.g. pack-index caching),
    /// not a fix for an observed bug.
    pub fn invalidate(&self, path: &Path) {
        self.repo_cache.invalidate(path);
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
        let repo_cache = self.repo_cache.clone();
        tokio::task::spawn_blocking(move || {
            let handle = Self::open_repo_with_cache(&repo_cache, &repo_path)?;
            repo_ops::read_files(&Self::thread_local_with_object_cache(&handle), &paths, &rev)
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
        let repo_cache = self.repo_cache.clone();
        tokio::task::spawn_blocking(move || {
            let handle = Self::open_repo_with_cache(&repo_cache, &repo_path)?;
            repo_ops::tip_commit(&Self::thread_local_with_object_cache(&handle), &rev)
        })
        .await?
    }

    /// Recursively list every directory reached while descending
    /// from `path_prefix`, each paired with the blob (file) names it
    /// holds directly, resolving the commit and target tree once
    /// inside a single `spawn_blocking`.
    ///
    /// Not part of [`GitBackend`] for the same reason as
    /// [`Self::read_files`]: this batches [`GitBackend::list_tree`]
    /// and [`GitBackend::list_subtrees`] into one round trip for a
    /// caller doing a full recursive descent, instead of the N
    /// separate calls (each re-resolving the commit and root tree
    /// from scratch) a manual DFS over those two trait methods would
    /// pay per directory node.
    pub async fn list_tree_recursive(
        &self,
        repo: &RepoHandle,
        path_prefix: &str,
        rev: &Rev,
    ) -> Result<Vec<repo_ops::TreeDirEntry>, GitError> {
        let repo_path = Self::handle_path(repo).to_path_buf();
        let prefix = path_prefix.to_string();
        let rev = rev.clone();
        let repo_cache = self.repo_cache.clone();
        tokio::task::spawn_blocking(move || {
            let handle = Self::open_repo_with_cache(&repo_cache, &repo_path)?;
            repo_ops::list_tree_recursive(
                &Self::thread_local_with_object_cache(&handle),
                &prefix,
                &rev,
            )
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

    /// `clone`/`fetch`/`push` shell out to the `git` binary for the actual network transfer.
    /// They are genuinely `async fn` built on `tokio::process::Command`.
    /// Each is bounded by its own `tokio::time::timeout` (see `crate::native::repo_ops`).
    /// Unlike every other `GitBackend` method here, they run directly on the calling task.
    /// They never run inside `spawn_blocking`.
    /// This path has no synchronous `gix`/filesystem work to offload.
    /// Wrapping them would only cost a blocking-pool thread for no benefit.
    async fn clone_to(
        &self,
        remote_url: &str,
        dst: &Path,
        creds: &Credentials,
    ) -> Result<(), GitError> {
        repo_ops::clone(remote_url, dst, creds).await
    }

    async fn fetch(
        &self,
        repo: &RepoHandle,
        remote_url: &str,
        refs: &[RefSpec],
        creds: &Credentials,
    ) -> Result<(), GitError> {
        let repo_path = Self::handle_path(repo);
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
        repo_ops::fetch(repo_path, remote_url, &refspecs, creds).await
    }

    async fn push(
        &self,
        repo: &RepoHandle,
        remote_url: &str,
        refs: &[RefSpec],
        creds: &Credentials,
    ) -> Result<PushReport, GitError> {
        let repo_path = Self::handle_path(repo).to_path_buf();
        let refspecs: Vec<(String, String, bool)> = refs
            .iter()
            .map(|r| (r.local.clone(), r.remote.clone(), r.force))
            .collect();
        let repo_cache = self.repo_cache.clone();
        // The local-ref preflight check needs the synchronous `gix`
        // view, so it still runs inside `spawn_blocking`; the actual
        // network push below no longer holds a `gix::Repository` at
        // all and awaits directly.
        let preflight_path = repo_path.clone();
        let preflight_refspecs = refspecs.clone();
        tokio::task::spawn_blocking(move || {
            let handle = Self::open_repo_with_cache(&repo_cache, &preflight_path)?;
            repo_ops::preflight_local_refs(
                &Self::thread_local_with_object_cache(&handle),
                &preflight_refspecs,
            )
        })
        .await??;
        repo_ops::push(&repo_path, remote_url, &refspecs, creds).await
    }

    async fn read_file(&self, repo: &RepoHandle, path: &str, rev: &Rev) -> Result<Bytes, GitError> {
        let repo_path = Self::handle_path(repo).to_path_buf();
        let path_owned = path.to_string();
        let rev = rev.clone();
        let repo_cache = self.repo_cache.clone();
        tokio::task::spawn_blocking(move || {
            let handle = Self::open_repo_with_cache(&repo_cache, &repo_path)?;
            repo_ops::read_file(
                &Self::thread_local_with_object_cache(&handle),
                &path_owned,
                &rev,
            )
        })
        .await?
    }

    async fn write_commit(&self, repo: &RepoHandle, spec: CommitSpec) -> Result<String, GitError> {
        let repo_path = Self::handle_path(repo).to_path_buf();
        let repo_cache = self.repo_cache.clone();
        tokio::task::spawn_blocking(move || {
            let handle = Self::open_repo_with_cache(&repo_cache, &repo_path)?;
            repo_ops::write_commit(&Self::thread_local_with_object_cache(&handle), spec)
        })
        .await?
    }

    async fn tag(&self, repo: &RepoHandle, name: &str, target: &str) -> Result<(), GitError> {
        let repo_path = Self::handle_path(repo).to_path_buf();
        let name = name.to_string();
        let target = target.to_string();
        let repo_cache = self.repo_cache.clone();
        tokio::task::spawn_blocking(move || {
            let handle = Self::open_repo_with_cache(&repo_cache, &repo_path)?;
            repo_ops::tag(
                &Self::thread_local_with_object_cache(&handle),
                &name,
                &target,
            )
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
        let repo_cache = self.repo_cache.clone();
        tokio::task::spawn_blocking(move || {
            let handle = Self::open_repo_with_cache(&repo_cache, &repo_path)?;
            repo_ops::fast_forward(
                &Self::thread_local_with_object_cache(&handle),
                &local_ref,
                &target_ref,
            )
        })
        .await?
    }

    async fn walk_history(
        &self,
        repo: &RepoHandle,
        path: &str,
        limit: Option<usize>,
    ) -> Result<Vec<CommitMeta>, GitError> {
        let repo_path = Self::handle_path(repo).to_path_buf();
        let path = path.to_string();
        let repo_cache = self.repo_cache.clone();
        tokio::task::spawn_blocking(move || {
            let handle = Self::open_repo_with_cache(&repo_cache, &repo_path)?;
            repo_ops::walk_history(&Self::thread_local_with_object_cache(&handle), &path, limit)
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
        let repo_cache = self.repo_cache.clone();
        tokio::task::spawn_blocking(move || {
            let handle = Self::open_repo_with_cache(&repo_cache, &repo_path)?;
            repo_ops::list_tree(
                &Self::thread_local_with_object_cache(&handle),
                &prefix,
                &rev,
            )
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
        let repo_cache = self.repo_cache.clone();
        tokio::task::spawn_blocking(move || {
            let handle = Self::open_repo_with_cache(&repo_cache, &repo_path)?;
            repo_ops::list_subtrees(
                &Self::thread_local_with_object_cache(&handle),
                &prefix,
                &rev,
            )
        })
        .await?
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    /// Mechanism-level falsification check for `invalidate`, since the
    /// behavioral integration tests (`crates/mmcp-git/tests/native_backend.rs`)
    /// cannot discriminate gix's current self-healing reads from a
    /// genuinely working eviction: assert directly against
    /// `repo_cache`'s contents instead of on behavior. Red check
    /// performed manually: emptying `invalidate`'s body (`let _ =
    /// path;` instead of `cache.remove(path);`) makes this fail with
    /// `assert!(!cache.contains_key...)` since the entry survives;
    /// restoring the real body makes it pass again.
    #[test]
    fn invalidate_evicts_cached_entry() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let backend = NativeBackend::new(tmp.path()).expect("backend");
        let path = tmp.path().join("nonexistent.git");

        // `gix::ThreadSafeRepository` has no cheap standalone test
        // constructor, so the cache is populated the real way: init a
        // bare repo and open it through `open_repo_with_cache`.
        let repo_path = backend.repo_path(uuid::Uuid::now_v7());
        std::fs::create_dir_all(&repo_path).expect("mkdir");
        gix::init_bare(&repo_path).expect("init bare");
        NativeBackend::open_repo_with_cache(&backend.repo_cache, &repo_path)
            .expect("populate cache");
        assert!(
            backend.repo_cache.contains_key(&repo_path),
            "precondition: open_repo must have cached the entry"
        );

        backend.invalidate(&repo_path);

        assert!(
            !backend.repo_cache.contains_key(&repo_path),
            "invalidate must remove the cached entry"
        );

        // A path never cached is a harmless no-op, not a panic.
        backend.invalidate(&path);
    }

    /// Mechanism-level falsification check for
    /// `thread_local_with_object_cache`: `gix::Repository::objects`
    /// exposes `has_object_cache()`, which is the only public signal
    /// the pinned `gix` fork gives for whether the decoded-object
    /// cache is actually configured (no hit-rate counter exists to
    /// assert against). Red check performed manually: removing the
    /// `repo.object_cache_size_if_unset(...)` call from
    /// `thread_local_with_object_cache` makes this fail with
    /// `assert!(repo.objects.has_object_cache())`; restoring the call
    /// makes it pass again.
    #[test]
    fn thread_local_with_object_cache_enables_the_cache() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let backend = NativeBackend::new(tmp.path()).expect("backend");
        let repo_path = backend.repo_path(uuid::Uuid::now_v7());
        std::fs::create_dir_all(&repo_path).expect("mkdir");
        gix::init_bare(&repo_path).expect("init bare");

        let handle = NativeBackend::open_repo_with_cache(&backend.repo_cache, &repo_path)
            .expect("open repo");
        let repo = NativeBackend::thread_local_with_object_cache(&handle);

        assert!(
            repo.objects.has_object_cache(),
            "thread_local_with_object_cache must leave the decoded-object cache enabled"
        );
    }
}
