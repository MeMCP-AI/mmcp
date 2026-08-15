//! In-memory index of every group repository cloned locally.
//!
//! The index is built by walking `~/.mmcp/repos/`,
//! treating each `<uuid>.git/` directory as one bare group repository,
//! opened through the [`NativeBackend`].
//! Reads the `.mmcp.toml` manifest at `HEAD:.mmcp.toml` to learn the group's slug, owner, and display name.
//!
//! The index is kept behind an `Arc<RwLock<_>>`,
//! so MCP tool handlers on the serve side and the file-watcher task (still in `mmcp-client`),
//! can share ownership without cloning the whole map.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use futures_util::StreamExt;
use futures_util::stream;
use jiff::Timestamp;
use mmcp_core::id::GroupId;
use mmcp_core::manifest::GroupManifest;
use mmcp_git::{GitBackend, NativeBackend, RepoHandle};
use tokio::sync::RwLock;
use uuid::Uuid;

use crate::defaults::MAX_CONCURRENT_MANIFEST_SCANS;
use crate::error::{FileOperation, StoreError};

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
        std::fs::create_dir_all(&repos_root).map_err(|source| StoreError::Io {
            path: repos_root.clone(),
            operation: FileOperation::CreateDir,
            source,
        })?;
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
    /// Returns `Some(scope)` when the group is indexed and the index is not being rewritten, `None` otherwise.
    /// The sync engine's `ScopeIndex::scope_of` impl goes through this helper for a non-blocking path.
    /// It runs inside an already-driving tokio runtime,
    /// where a `block_on` would panic with "Cannot start a runtime from within a runtime".
    /// The index's `RwLock` is never held across an await point and refresh is rare, so `try_read` almost always succeeds.
    /// When it does not, the engine treats the result as "unknown" and the caller retries on its next tick.
    #[must_use]
    pub fn try_scope_of(&self, group_id: &GroupId) -> Option<mmcp_core::manifest::GroupScope> {
        self.inner
            .try_read()
            .ok()
            .and_then(|guard| guard.get(group_id).map(|entry| entry.manifest.scope))
    }

    /// Non-blocking snapshot of every indexed group's UUID.
    ///
    /// Mirrors [`try_scope_of`]: the sync engine's `GroupHandleResolver::iter_group_ids` calls this from an async worker.
    /// No `block_on` bridge is available there, so this stays non-blocking.
    /// Returns an empty vector when the lock is held by a writer.
    /// Callers treat that as "no groups" and retry on the next tick.
    #[must_use]
    pub fn try_list_ids(&self) -> Vec<Uuid> {
        self.inner
            .try_read()
            .ok()
            .map(|guard| guard.keys().map(|gid| *gid.as_uuid()).collect())
            .unwrap_or_default()
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

    /// Rebuild the index from the filesystem.
    /// Called on startup and whenever the watcher reports that the repos root changed.
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
/// Directories that look like group repos but are missing a manifest, or whose manifest fails to parse,
/// are logged at `warn` level and skipped.
/// The scan never fails the whole index because of a single broken repo.
async fn scan_repos_root(
    repos_root: &Path,
    backend: &NativeBackend,
) -> Result<Vec<GroupEntry>, StoreError> {
    let read_dir = match std::fs::read_dir(repos_root) {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => {
            return Err(StoreError::Io {
                path: repos_root.to_path_buf(),
                operation: FileOperation::ReadDir,
                source,
            });
        }
    };

    // Phase 1: walk the directory synchronously (cheap, no I/O beyond
    // the listing itself already paid for by `read_dir`) and collect
    // every candidate `<uuid>.git` directory's handle.
    let mut candidates = Vec::new();
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
        candidates.push((dir_name, uuid, handle));
    }

    // Phase 2: read every candidate's manifest with bounded
    // concurrency instead of one `read_manifest` after another.
    // Warn-and-skip semantics for an unreadable manifest or an
    // id/directory mismatch are byte-identical to the previous serial
    // loop; only how many run at once changed.
    let entries: Vec<GroupEntry> = stream::iter(candidates)
        .map(|(dir_name, uuid, handle)| async move {
            let manifest = match backend.read_manifest(&handle).await {
                Ok(m) => m,
                Err(err) => {
                    tracing::warn!(
                        dir = %dir_name,
                        error = %err,
                        "group repo missing or unreadable manifest, skipping"
                    );
                    return None;
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
                return None;
            }

            Some(GroupEntry {
                handle,
                manifest,
                last_rescan: Timestamp::now().as_millisecond(),
            })
        })
        .buffer_unordered(MAX_CONCURRENT_MANIFEST_SCANS)
        .filter_map(std::future::ready)
        .collect()
        .await;
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mmcp_core::id::UserId;
    use tempfile::TempDir;

    /// A directory shaped like a group repo (`<uuid>.git`) but with no
    /// git repo inside it at all: `read_manifest` fails, and the
    /// bounded-concurrency scan must still warn-and-skip it,
    /// without failing the whole refresh or dropping any of the good repos scanned alongside it.
    #[tokio::test]
    async fn refresh_tolerates_one_broken_repo_among_several_good_ones() {
        let tmp = TempDir::new().expect("tempdir");
        let repos_root = tmp.path().join("repos");
        let backend = NativeBackend::new(&repos_root).expect("backend");

        let mut good_ids = Vec::new();
        for i in 0..5 {
            let group_id = GroupId::new();
            let manifest =
                GroupManifest::new_user_owned(group_id, format!("group-{i}"), UserId::new());
            backend
                .create_group_repo(&manifest)
                .await
                .expect("create group repo");
            good_ids.push(group_id);
        }

        let broken_uuid = Uuid::now_v7();
        std::fs::create_dir_all(repos_root.join(format!("{broken_uuid}.git")))
            .expect("mkdir broken repo dir");

        let index = GroupIndex::build(repos_root, Arc::new(backend))
            .await
            .expect("build must tolerate the broken repo");

        assert_eq!(
            index.len().await,
            good_ids.len(),
            "only the good repos should be indexed"
        );
        for group_id in good_ids {
            assert!(
                index.get(&group_id).await.is_some(),
                "every good repo must still be indexed"
            );
        }
    }
}
