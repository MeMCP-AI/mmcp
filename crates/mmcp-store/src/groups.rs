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
use tokio::sync::{Mutex, RwLock};
use uuid::Uuid;

use crate::defaults::MAX_CONCURRENT_MANIFEST_SCANS;
use crate::error::{FileOperation, StoreError};

/// [`GroupIndex`]'s backing lock was held by a concurrent writer when
/// a non-blocking lookup ran.
///
/// Transient by construction: [`GroupIndex::refresh`] never holds the
/// lock across an await point, so this clears on the caller's next
/// attempt. Distinct from "genuinely not indexed" (`Ok(None)` on
/// [`GroupIndex::try_get`]): a caller that cannot tell the two apart
/// cannot tell "retry, this will resolve itself" from "real problem,
/// investigate" from the reported failure alone.
#[derive(Debug, Clone, Copy, thiserror::Error)]
#[error("group index lookup was contended by a concurrent writer")]
pub struct GroupIndexContended;

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

impl GroupEntry {
    /// Build a `GroupEntry` from its three fields.
    #[must_use]
    pub fn new(handle: RepoHandle, manifest: GroupManifest, last_rescan: i64) -> Self {
        Self {
            handle,
            manifest,
            last_rescan,
        }
    }
}

/// Thread-safe map from [`GroupId`] to [`GroupEntry`].
#[derive(Clone)]
pub struct GroupIndex {
    repos_root: PathBuf,
    backend: Arc<NativeBackend>,
    inner: Arc<RwLock<HashMap<GroupId, GroupEntry>>>,
    /// Held by a refresh from the start of its scan to the end of its install.
    /// Never held by a reader, and distinct from `inner`, which is never held across an await point.
    refresh_serializer: Arc<Mutex<()>>,
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
        std::fs::create_dir_all(&repos_root).map_err(|source| {
            StoreError::io(repos_root.clone(), FileOperation::CreateDir, source)
        })?;
        let index = Self {
            repos_root,
            backend,
            inner: Arc::new(RwLock::new(HashMap::new())),
            refresh_serializer: Arc::new(Mutex::new(())),
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

    /// Non-blocking entry lookup for use from sync contexts.
    ///
    /// Mirrors [`Self::try_scope_of`]: the sync engine's `GroupHandleResolver::resolve` impl
    /// goes through this helper instead of bridging the async [`get`](Self::get) with
    /// `Handle::block_on`, which panics with "Cannot start a runtime from within a
    /// runtime" whenever the calling thread has already entered a tokio runtime context
    /// (every production caller: `push_one_group` and friends are themselves async fns
    /// driven by the multi-thread runtime `mmcp-server`/`mmcp-client` start under).
    ///
    /// `Ok(None)`: the group is genuinely not indexed. `Err(GroupIndexContended)`:
    /// the lock is currently held by a writer (`refresh` is rare and never holds
    /// the lock across an await point, so this is uncommon). Kept distinct, rather
    /// than both collapsing to `None`, so a caller can tell a transient race from
    /// a real "this group does not exist" result.
    pub fn try_get(&self, group_id: &GroupId) -> Result<Option<GroupEntry>, GroupIndexContended> {
        self.inner
            .try_read()
            .map(|guard| guard.get(group_id).cloned())
            .map_err(|_| GroupIndexContended)
    }

    /// Non-blocking snapshot of every indexed group's UUID.
    ///
    /// Mirrors [`Self::try_scope_of`].
    /// Safe to call from an async worker with no `block_on` bridge available.
    /// Returns an empty vector when the lock is held by a writer.
    /// Callers treat that as "no groups" and retry on the next tick.
    /// Ordered as [`Self::list`] orders its entries.
    #[must_use]
    pub fn try_list_ids(&self) -> Vec<Uuid> {
        self.inner
            .try_read()
            .ok()
            .map(|guard| {
                sorted_entries(&guard)
                    .into_iter()
                    .map(|entry| *entry.manifest.group_id.as_uuid())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Snapshot of every entry in the index, ordered by manifest slug, then by group UUID.
    ///
    /// The order is total and independent of insertion order.
    /// A slug is not unique across owners, so the UUID breaks ties.
    /// A refresh changes the order only when the set of indexed groups or a slug changes.
    /// A group whose manifest cannot be read is not indexed, so it leaves the set until a refresh reads it again.
    /// Offset paging over successive snapshots can repeat or skip a group across such a change.
    pub async fn list(&self) -> Vec<GroupEntry> {
        let guard = self.inner.read().await;
        sorted_entries(&guard).into_iter().cloned().collect()
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
    ///
    /// Refreshes are serialized end to end: a refresh starts its scan only after the previous one finished.
    /// An older scan therefore never installs after a newer one.
    /// Readers are not blocked while a scan runs.
    pub async fn refresh(&self) -> Result<(), StoreError> {
        let _serialized = self.refresh_serializer.lock().await;
        let by_id = scan_repos_root(&self.repos_root, self.backend.as_ref())
            .await?
            .into_iter()
            .map(|entry| {
                (
                    GroupId::from_uuid(*entry.manifest.group_id.as_uuid()),
                    entry,
                )
            })
            .collect();
        *self.inner.write().await = by_id;
        Ok(())
    }
}

/// Position of `left` relative to `right` in [`GroupIndex::list`]: manifest slug, then group UUID.
fn compare_group_entries(left: &GroupEntry, right: &GroupEntry) -> std::cmp::Ordering {
    let left_key = (&left.manifest.slug, left.manifest.group_id.as_uuid());
    let right_key = (&right.manifest.slug, right.manifest.group_id.as_uuid());
    left_key.cmp(&right_key)
}

/// Entries of `groups` in the order [`GroupIndex::list`] documents.
fn sorted_entries(groups: &HashMap<GroupId, GroupEntry>) -> Vec<&GroupEntry> {
    let mut entries: Vec<&GroupEntry> = groups.values().collect();
    entries.sort_by(|left, right| compare_group_entries(left, right));
    entries
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
            return Err(StoreError::io(
                repos_root.to_path_buf(),
                FileOperation::ReadDir,
                source,
            ));
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

            Some(GroupEntry::new(
                handle,
                manifest,
                Timestamp::now().as_millisecond(),
            ))
        })
        .buffer_unordered(MAX_CONCURRENT_MANIFEST_SCANS)
        .filter_map(std::future::ready)
        .collect()
        .await;
    Ok(entries)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use futures_util::FutureExt;
    use mmcp_core::id::UserId;
    use tempfile::TempDir;

    /// Distinct slugs seeded by the ordering test.
    /// Enough that an unordered map matching slug order by chance is negligible.
    const DISTINCT_SLUG_COUNT: usize = 16;

    /// Coprime with [`DISTINCT_SLUG_COUNT`].
    /// Stepping by it visits every slug exactly once in a non-sorted order.
    const CREATION_SLUG_STRIDE: usize = 7;

    /// Slug carried by two groups of the ordering test, to exercise the UUID tie-break.
    const SHARED_SLUG: &str = "group-03";

    /// Create `slug`'s group repository under `backend`; returns its id.
    async fn create_group(backend: &NativeBackend, slug: &str) -> GroupId {
        let group_id = GroupId::new();
        let manifest = GroupManifest::new_user_owned(group_id, slug, UserId::new());
        backend
            .create_group_repo(&manifest)
            .await
            .expect("create group repo");
        group_id
    }

    /// An index entry carrying `slug` and a fresh group id, with no repository behind it.
    fn entry_with_slug(slug: &str) -> GroupEntry {
        let group_id = GroupId::new();
        let manifest = GroupManifest::new_user_owned(group_id, slug, UserId::new());
        GroupEntry::new(
            RepoHandle::new(*group_id.as_uuid(), String::new()),
            manifest,
            0,
        )
    }

    /// Two entries in ascending group UUID order, both carrying `slug`.
    fn entry_pair_by_ascending_uuid(slug: &str) -> [GroupEntry; 2] {
        let mut pair = [entry_with_slug(slug), entry_with_slug(slug)];
        pair.sort_by_key(|entry| *entry.manifest.group_id.as_uuid());
        pair
    }

    /// Entries with equal slugs are ordered by ascending group UUID.
    #[test]
    fn compare_group_entries_breaks_a_slug_tie_by_uuid() {
        let [lower_uuid, higher_uuid] = entry_pair_by_ascending_uuid(SHARED_SLUG);
        assert_eq!(
            compare_group_entries(&lower_uuid, &higher_uuid),
            std::cmp::Ordering::Less
        );
        assert_eq!(
            compare_group_entries(&higher_uuid, &lower_uuid),
            std::cmp::Ordering::Greater
        );
    }

    /// The slug decides before the UUID: the entry with the lower slug comes first even with the higher UUID.
    #[test]
    fn compare_group_entries_ranks_the_slug_before_the_uuid() {
        let [mut lower_uuid, mut higher_uuid] = entry_pair_by_ascending_uuid(SHARED_SLUG);
        lower_uuid.manifest.slug = "group-99".to_owned();
        higher_uuid.manifest.slug = "group-00".to_owned();
        assert_eq!(
            compare_group_entries(&higher_uuid, &lower_uuid),
            std::cmp::Ordering::Less
        );
        assert_eq!(
            compare_group_entries(&lower_uuid, &higher_uuid),
            std::cmp::Ordering::Greater
        );
    }

    /// `list` and `try_list_ids` order entries by slug, then UUID, whatever the creation order.
    /// A refresh that rescans the same groups leaves that order untouched.
    #[tokio::test]
    async fn list_orders_by_slug_then_uuid_across_refreshes() {
        let tmp = TempDir::new().expect("tempdir");
        let repos_root = tmp.path().join("repos");
        let backend = NativeBackend::new(&repos_root).expect("backend");

        let mut expected_order: Vec<(String, Uuid)> = Vec::new();
        for step in 0..DISTINCT_SLUG_COUNT {
            let slug = format!(
                "group-{:02}",
                (step * CREATION_SLUG_STRIDE) % DISTINCT_SLUG_COUNT
            );
            let group_id = create_group(&backend, &slug).await;
            expected_order.push((slug, *group_id.as_uuid()));
        }
        let duplicate_id = create_group(&backend, SHARED_SLUG).await;
        expected_order.push((SHARED_SLUG.to_owned(), *duplicate_id.as_uuid()));
        expected_order.sort();

        let index = GroupIndex::build(repos_root, Arc::new(backend))
            .await
            .expect("build");

        let listed_order = |entries: Vec<GroupEntry>| -> Vec<(String, Uuid)> {
            entries
                .into_iter()
                .map(|entry| (entry.manifest.slug, *entry.manifest.group_id.as_uuid()))
                .collect()
        };
        assert_eq!(
            listed_order(index.list().await),
            expected_order,
            "list must be ordered by slug, then UUID"
        );
        let expected_ids: Vec<Uuid> = expected_order.iter().map(|(_, uuid)| *uuid).collect();
        assert_eq!(
            index.try_list_ids(),
            expected_ids,
            "try_list_ids must follow the order of list"
        );

        index.refresh().await.expect("refresh");
        assert_eq!(
            listed_order(index.list().await),
            expected_order,
            "a refresh over the same groups must not reorder the listing"
        );
    }

    /// Blocking threads of the runtime built by the overlapping-refresh test.
    /// With one, a task holding it keeps every later `spawn_blocking` task queued.
    const SINGLE_BLOCKING_THREAD: usize = 1;

    /// Two production `refresh` calls overlap: the later one still indexes a group created after it was called.
    /// The test occupies its runtime's only blocking thread, so the manifest read of the earlier call stays queued.
    /// The earlier call is then held in its scan, holding the serializer, with the root listed.
    /// The later call is polled once and a second group is created afterwards.
    /// Both calls then run to completion and the second group must be indexed:
    /// the later call must scan only after the earlier one finished.
    /// No sleep and no timing: every step is a single poll or a release of the held thread.
    #[test]
    fn overlapping_refreshes_scan_one_after_the_other() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .max_blocking_threads(SINGLE_BLOCKING_THREAD)
            .build()
            .expect("current-thread runtime");
        runtime.block_on(async {
            let tmp = TempDir::new().expect("tempdir");
            let repos_root = tmp.path().join("repos");
            let backend = Arc::new(NativeBackend::new(&repos_root).expect("backend"));
            let first_group = create_group(&backend, "first-group").await;
            let index = GroupIndex::build(repos_root, Arc::clone(&backend))
                .await
                .expect("build");

            // A dropped sender also ends the held task, so a failed assertion never hangs the runtime shutdown.
            let (release_blocking_thread, held_blocking_thread) = std::sync::mpsc::channel::<()>();
            let blocking_thread_holder = tokio::task::spawn_blocking(move || {
                let _ = held_blocking_thread.recv();
            });

            let earlier_refresh = index.refresh();
            tokio::pin!(earlier_refresh);
            assert!(
                earlier_refresh.as_mut().now_or_never().is_none(),
                "the earlier refresh is held at its queued manifest read"
            );
            let later_refresh = index.refresh();
            tokio::pin!(later_refresh);
            assert!(
                later_refresh.as_mut().now_or_never().is_none(),
                "the later refresh cannot finish before the earlier one"
            );

            release_blocking_thread
                .send(())
                .expect("the holder is waiting");
            blocking_thread_holder.await.expect("holder task");
            let second_group = create_group(&backend, "second-group").await;

            let (earlier_result, later_result) = tokio::join!(earlier_refresh, later_refresh);
            earlier_result.expect("earlier refresh");
            later_result.expect("later refresh");
            assert!(
                index.get(&first_group).await.is_some(),
                "the group created before both calls is indexed"
            );
            assert!(
                index.get(&second_group).await.is_some(),
                "the later refresh must scan after the earlier one finished"
            );
        });
    }

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
