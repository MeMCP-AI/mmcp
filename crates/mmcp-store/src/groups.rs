//! In-memory index of every group repository cloned locally.
//!
//! The index is built by walking `~/.mmcp/repos/`, treating each
//! `<uuid>.git/` directory as one bare group repository, opening it
//! through the [`NativeBackend`], and reading the `.mmcp.toml`
//! manifest at `HEAD:.mmcp.toml` to learn the group's slug, owner,
//! and display name.
//!
//! The index is kept behind an `Arc<RwLock<_>>` so MCP tool
//! handlers on the serve side and the file-watcher task (still in
//! `mmcp-client`) can share ownership without cloning the whole
//! map.
//!
//! History: ported from `crates/mmcp-client/src/state/groups.rs`
//! during the FR-020 extraction.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use jiff::Timestamp;
use mmcp_core::id::GroupId;
use mmcp_core::manifest::GroupManifest;
use mmcp_git::{GitBackend, NativeBackend, RepoHandle};
use tokio::sync::RwLock;
use uuid::Uuid;

use crate::error::StoreError;

/// One entry in the [`GroupIndex`].
#[derive(Debug, Clone)]
pub struct GroupEntry {
    /// Handle the backend uses to open the repo.
    pub handle: RepoHandle,

    /// Manifest parsed from `HEAD:.mmcp.toml`.
    pub manifest: GroupManifest,

    /// When the entry was last (re-)scanned, ms since epoch.
    /// Currently unread; reserved for staleness hints in `diagnose`.
    #[allow(dead_code)]
    pub last_rescan: i64,
}

/// Thread-safe map from [`GroupId`] to [`GroupEntry`].
#[derive(Clone)]
pub struct GroupIndex {
    repos_root: PathBuf,
    backend: Arc<NativeBackend>,
    inner: Arc<RwLock<HashMap<GroupId, GroupEntry>>>,
}

impl GroupIndex {
    /// Build a fresh index by walking `repos_root`.
    ///
    /// The repos root is created if it does not yet exist so a
    /// first-run client starts with an empty index rather than an
    /// error.
    pub async fn build(
        repos_root: PathBuf,
        backend: Arc<NativeBackend>,
    ) -> Result<Self, StoreError> {
        std::fs::create_dir_all(&repos_root)
            .map_err(|e| StoreError::Io(format!("create {}: {e}", repos_root.display())))?;
        let index = Self {
            repos_root,
            backend,
            inner: Arc::new(RwLock::new(HashMap::new())),
        };
        index.refresh().await?;
        Ok(index)
    }

    /// Return the entry for `group_id` if the local mirror has it.
    pub async fn get(&self, group_id: &GroupId) -> Option<GroupEntry> {
        self.inner.read().await.get(group_id).cloned()
    }

    /// Non-blocking scope lookup for use from sync contexts.
    ///
    /// Returns `Some(scope)` when the group is indexed and the
    /// index is not being rewritten; `None` when either condition
    /// fails. The sync engine's `ScopeIndex::scope_of` impl goes
    /// through this helper, which needs a non-blocking path
    /// because it runs inside an already-driving tokio runtime and
    /// a `block_on` would panic with "Cannot start a runtime from
    /// within a runtime". The index's `RwLock` is never held
    /// across an await point and refresh is rare, so `try_read`
    /// almost always succeeds; when it does not, the engine treats
    /// the result as "unknown" and the caller retries on its next
    /// tick.
    #[must_use]
    pub fn try_scope_of(&self, group_id: &GroupId) -> Option<mmcp_core::manifest::GroupScope> {
        self.inner
            .try_read()
            .ok()
            .and_then(|guard| guard.get(group_id).map(|entry| entry.manifest.scope))
    }

    /// Snapshot of every entry in the index.
    pub async fn list(&self) -> Vec<GroupEntry> {
        self.inner.read().await.values().cloned().collect()
    }

    /// Number of groups currently indexed.
    #[allow(dead_code)] // consumed by `mmcp status` and test helpers landing in later phases.
    pub async fn len(&self) -> usize {
        self.inner.read().await.len()
    }

    /// True if the index is empty.
    #[allow(dead_code)]
    pub async fn is_empty(&self) -> bool {
        self.inner.read().await.is_empty()
    }

    /// Rebuild the index from the filesystem. Called on startup and
    /// whenever the watcher reports that the repos root changed.
    pub async fn refresh(&self) -> Result<(), StoreError> {
        let entries = scan_repos_root(&self.repos_root, self.backend.as_ref()).await?;
        let mut guard = self.inner.write().await;
        guard.clear();
        for entry in entries {
            guard.insert(
                GroupId::from_uuid(*entry.manifest.group_id.as_uuid()),
                entry,
            );
        }
        Ok(())
    }
}

/// Walk `repos_root` and return one [`GroupEntry`] per `<uuid>.git`
/// directory that has a readable manifest on the `main` branch.
///
/// Directories that look like group repos but are missing a
/// manifest, or whose manifest fails to parse, are logged at
/// `warn` level and skipped. The scan never fails the whole index
/// because of a single broken repo.
async fn scan_repos_root(
    repos_root: &Path,
    backend: &NativeBackend,
) -> Result<Vec<GroupEntry>, StoreError> {
    let read_dir = match std::fs::read_dir(repos_root) {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => {
            return Err(StoreError::Io(format!(
                "read_dir {}: {e}",
                repos_root.display()
            )));
        }
    };

    let mut entries = Vec::new();
    for item in read_dir {
        let item = match item {
            Ok(i) => i,
            Err(e) => {
                tracing::warn!(error = %e, "skipping repos_root entry with read error");
                continue;
            }
        };
        let path = item.path();
        if !path.is_dir() {
            continue;
        }
        let dir_name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };
        let Some(uuid_str) = dir_name.strip_suffix(".git") else {
            continue;
        };
        let uuid = match Uuid::parse_str(uuid_str) {
            Ok(u) => u,
            Err(_) => {
                tracing::warn!(dir = %dir_name, "ignoring directory with non-uuid name");
                continue;
            }
        };

        let handle = RepoHandle::new(uuid, path.to_string_lossy().into_owned());
        let manifest = match backend.read_manifest(&handle).await {
            Ok(m) => m,
            Err(err) => {
                tracing::warn!(
                    dir = %dir_name,
                    error = %err,
                    "group repo missing or unreadable manifest, skipping"
                );
                continue;
            }
        };

        // The directory name must match the manifest's group id,
        // otherwise we have a drift problem worth flagging.
        if manifest.group_id.as_uuid() != &uuid {
            tracing::warn!(
                dir = %dir_name,
                manifest_id = %manifest.group_id,
                "manifest group id does not match directory name, skipping"
            );
            continue;
        }

        entries.push(GroupEntry {
            handle,
            manifest,
            last_rescan: Timestamp::now().as_millisecond(),
        });
    }
    Ok(entries)
}
