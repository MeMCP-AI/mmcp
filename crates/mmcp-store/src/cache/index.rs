//! Populate the cache: full rebuilds and single-record upserts.

use jiff::Timestamp;
use mmcp_core::memory::MemoryFile;
use mmcp_git::{GitBackend, NativeBackend, Rev};
use sqlx::Sqlite;
use sqlx::sqlite::SqlitePool;
use uuid::Uuid;

use crate::groups::{GroupEntry, GroupIndex};
use crate::memory::list_all_memory_files;

use super::{CacheError, IndexedRecord};

/// Outcome of a [`rebuild_full`] run.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RebuildStats {
    pub groups_scanned: usize,
    pub memories_indexed: usize,
}

/// Rebuild the whole cache from scratch: every memory in every locally-mirrored group,
/// read at each group's current `HEAD`.
/// Existing rows are cleared first so a memory deleted or moved upstream does not linger as a stale hit.
/// This is both the lazy-build-on-read primitive (see [`super::query`]),
/// and the body of the debug/admin "force rebuild" entry point.
///
/// The clear, the repopulate loop, and the `mark_built` stamp all run inside ONE sqlite transaction.
/// A partial failure (e.g. a git error walking one group) rolls the whole attempt back,
/// rather than leaving `indexed_memory` truncated while [`super::schema::is_built`],
/// still reports the PREVIOUS build as current:
/// readers keep serving the last known-good index instead of a half-emptied one,
/// and the build-completion flag never observably disagrees with the row data it describes.
pub async fn rebuild_full(
    pool: &SqlitePool,
    backend: &NativeBackend,
    groups: &GroupIndex,
) -> Result<RebuildStats, CacheError> {
    let mut stats = RebuildStats::default();
    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM indexed_memory")
        .execute(&mut *tx)
        .await?;

    for entry in groups.list().await {
        stats.groups_scanned += 1;
        stats.memories_indexed += index_group(&mut tx, backend, &entry).await?;
    }

    let now = Timestamp::now().to_string();
    super::schema::mark_built(&mut *tx, &now).await?;
    tx.commit().await?;
    Ok(stats)
}

/// Re-index every memory in exactly the groups named by `group_ids`,
/// leaving every other group's rows untouched.
/// Used by the sync pull-trigger hook (see `mmcp-client`'s `commands/sync.rs` and `commands/serve.rs`):
/// re-walking only the groups a pull actually advanced is cheap and precise,
/// unlike [`rebuild_full`]'s whole-mirror sweep.
/// A no-op (not an error) if the index has never completed its first build:
/// a pull-triggered partial update on top of a never-built cache,
/// would leave every other local group looking indexed when it is not;
/// the next read simply pays for the full lazy build instead.
///
/// Every requested group's clear-and-repopulate runs inside ONE sqlite transaction,
/// same rationale as [`rebuild_full`]:
/// a git error walking one group rolls back every group already deleted/repopulated in this call,
/// so the other requested groups are never left with truncated rows,
/// while the untouched [`super::schema::is_built`] flag keeps claiming a fully-built index.
pub async fn rebuild_groups(
    pool: &SqlitePool,
    backend: &NativeBackend,
    groups: &GroupIndex,
    group_ids: &[Uuid],
) -> Result<RebuildStats, CacheError> {
    let mut stats = RebuildStats::default();
    if group_ids.is_empty() || !super::schema::is_built(pool).await? {
        return Ok(stats);
    }
    let mut tx = pool.begin().await?;
    for group_id in group_ids {
        let Some(entry) = groups
            .get(&mmcp_core::id::GroupId::from_uuid(*group_id))
            .await
        else {
            // Advertised by the pull report but not (yet) resolvable
            // through the live index -- benign race with a
            // concurrent index refresh; the next lazy or debug
            // rebuild picks it up.
            continue;
        };
        sqlx::query("DELETE FROM indexed_memory WHERE group_id = ?")
            .bind(group_id.to_string())
            .execute(&mut *tx)
            .await?;
        stats.groups_scanned += 1;
        stats.memories_indexed += index_group(&mut tx, backend, &entry).await?;
    }
    tx.commit().await?;
    Ok(stats)
}

/// Read and upsert every memory currently in `entry`'s group at `HEAD`.
/// Returns the number of memories indexed.
/// Shared body for [`rebuild_full`] and [`rebuild_groups`];
/// callers own clearing any stale rows for the group before calling this,
/// and own committing or rolling back `tx`.
async fn index_group(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    backend: &NativeBackend,
    entry: &GroupEntry,
) -> Result<usize, CacheError> {
    let mut indexed = 0usize;
    let rev = Rev::head();
    let files = list_all_memory_files(backend, &entry.handle, &rev).await?;
    for file_ref in files {
        let bytes = match backend.read_file(&entry.handle, &file_ref.path, &rev).await {
            Ok(bytes) => bytes,
            // Raced with a concurrent delete/move between the
            // listing and the read: skip rather than fail the whole
            // rebuild over one vanished file.
            Err(mmcp_git::GitError::PathNotFound(_)) => continue,
            Err(err) => return Err(CacheError::Git(err)),
        };
        let text = String::from_utf8_lossy(&bytes);
        let Ok(memory_file) = MemoryFile::parse(&text) else {
            // A corrupt or non-conforming memory file should not
            // sink the whole rebuild; `diagnose` / `check_health`
            // already own surfacing parse errors as first-class
            // findings, the cache just skips it.
            continue;
        };
        let record = build_record(
            entry.handle.group_id,
            file_ref.id,
            &file_ref.slug,
            &file_ref.path,
            "HEAD",
            &memory_file,
        );
        upsert_record(&mut **tx, &record).await?;
        indexed += 1;
    }
    Ok(indexed)
}

/// Build an [`IndexedRecord`] from a parsed memory file plus the
/// addressing metadata the caller already resolved (group, slug,
/// filename id, repo-relative path, and the commit the content was
/// read at).
#[must_use]
pub fn build_record(
    group_id: Uuid,
    id: Uuid,
    slug: &str,
    path: &str,
    commit_id: &str,
    memory_file: &MemoryFile,
) -> IndexedRecord {
    let status = memory_file
        .frontmatter
        .feature
        .as_ref()
        .map(|f| f.status.as_str().to_string());
    let milestone = memory_file
        .frontmatter
        .feature
        .as_ref()
        .and_then(|f| f.milestone);
    IndexedRecord {
        group_id,
        id,
        slug: slug.to_string(),
        kind: memory_file.frontmatter.kind.as_str().to_string(),
        name: memory_file.frontmatter.name.clone(),
        description: memory_file.frontmatter.description.clone(),
        tags: memory_file.frontmatter.tags.clone(),
        body: memory_file.body.clone(),
        path: path.to_string(),
        commit_id: commit_id.to_string(),
        status,
        milestone,
    }
}

/// Insert or replace one memory's row, recomputing its embedding (see [`super::embed`]),
/// from the current name, description, tags, and body every time:
/// an edit that changes the text must not leave a stale embedding behind.
///
/// Generic over the executor so [`index_group`] can run every upsert inside the caller's transaction,
/// (see [`rebuild_full`] and [`rebuild_groups`]),
/// while the write-trigger hook ([`super::notify_write`]) keeps passing the plain pool.
pub async fn upsert_record<'e, E>(executor: E, record: &IndexedRecord) -> Result<(), CacheError>
where
    E: sqlx::Executor<'e, Database = Sqlite>,
{
    let tags_json = serde_json::to_string(&record.tags).unwrap_or_default();
    let now = Timestamp::now().to_string();
    let embedding_text = format!(
        "{} {} {} {}",
        record.name,
        record.description,
        record.tags.join(" "),
        record.body
    );
    let embedding = super::embed::vector_to_bytes(&super::embed::embed_text(&embedding_text));
    let milestone = record.milestone.map(|id| id.to_string());
    sqlx::query(
        "INSERT INTO indexed_memory \
           (group_id, id, slug, kind, name, description, tags, body, path, commit_id, embedding, updated_at, status, milestone) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
         ON CONFLICT(group_id, id) DO UPDATE SET \
           slug = excluded.slug, kind = excluded.kind, name = excluded.name, \
           description = excluded.description, tags = excluded.tags, body = excluded.body, \
           path = excluded.path, commit_id = excluded.commit_id, embedding = excluded.embedding, \
           updated_at = excluded.updated_at, status = excluded.status, milestone = excluded.milestone",
    )
    .bind(record.group_id.to_string())
    .bind(record.id.to_string())
    .bind(&record.slug)
    .bind(&record.kind)
    .bind(&record.name)
    .bind(&record.description)
    .bind(tags_json)
    .bind(&record.body)
    .bind(&record.path)
    .bind(&record.commit_id)
    .bind(embedding)
    .bind(now)
    .bind(&record.status)
    .bind(milestone)
    .execute(executor)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::ScratchHome;

    fn sample_memory_file(name: &str) -> MemoryFile {
        let source = format!(
            "+++\nname = \"{name}\"\ndescription = \"a test memory about {name}\"\nkind = \"scratch\"\n+++\n\nbody text for {name}\n"
        );
        MemoryFile::parse(&source).expect("parse sample memory")
    }

    #[tokio::test]
    async fn rebuild_full_indexes_every_local_memory() {
        let scratch = ScratchHome::new().await.expect("scratch home");
        let seeded = scratch
            .seed_group("cache-rebuild-test")
            .await
            .expect("seed group");
        let entry = scratch
            .groups()
            .get(&seeded.group_id)
            .await
            .expect("seeded group present");

        for name in ["alpha", "beta"] {
            let file = sample_memory_file(name);
            let rendered = file.to_string().expect("render memory file");
            let id = mmcp_core::id::MemoryId::new();
            let path = mmcp_core::conventions::memory_path(name, id);
            crate::memory::write_file_at_path(
                scratch.backend(),
                &entry.handle,
                &path,
                &rendered,
                scratch.author(),
                crate::memory::WriteFileOptions::default(),
            )
            .await
            .expect("write sample memory");
        }

        let tmp = tempfile::TempDir::new().expect("tempdir");
        let pool = super::super::open_pool(&tmp.path().join("index.sqlite3"))
            .await
            .expect("open pool");

        let stats = rebuild_full(&pool, scratch.backend(), scratch.groups())
            .await
            .expect("rebuild_full");
        assert_eq!(stats.groups_scanned, 1);
        assert_eq!(stats.memories_indexed, 2);
        assert!(
            super::super::schema::is_built(&pool)
                .await
                .expect("is_built")
        );

        let row_count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM indexed_memory")
            .fetch_one(&pool)
            .await
            .expect("count rows");
        assert_eq!(row_count.0, 2);
    }

    #[tokio::test]
    async fn rebuild_full_rolls_back_instead_of_leaving_a_truncated_built_index() {
        let scratch = ScratchHome::new().await.expect("scratch home");

        let good = scratch
            .seed_group("cache-atomic-good")
            .await
            .expect("seed good group");
        let good_entry = scratch
            .groups()
            .get(&good.group_id)
            .await
            .expect("good entry present");
        let file = sample_memory_file("alpha");
        let rendered = file.to_string().expect("render memory file");
        let id = mmcp_core::id::MemoryId::new();
        let path = mmcp_core::conventions::memory_path("alpha", id);
        crate::memory::write_file_at_path(
            scratch.backend(),
            &good_entry.handle,
            &path,
            &rendered,
            scratch.author(),
            crate::memory::WriteFileOptions::default(),
        )
        .await
        .expect("write sample memory");

        let tmp = tempfile::TempDir::new().expect("tempdir");
        let pool = super::super::open_pool(&tmp.path().join("index.sqlite3"))
            .await
            .expect("open pool");

        // Prime the index with one real, successful build so
        // `is_built` starts true and `indexed_memory` starts with a
        // known-good row: the assertion below must prove this state
        // survives untouched, not merely that a cold index stays cold.
        rebuild_full(&pool, scratch.backend(), scratch.groups())
            .await
            .expect("initial rebuild_full");
        assert!(
            super::super::schema::is_built(&pool)
                .await
                .expect("is_built after initial build")
        );

        // Seed a second group, then corrupt its bare repo on disk
        // after the group index already resolved it -- this forces
        // `index_group` to fail partway through the next rebuild
        // with a real git error instead of the benign
        // `PathNotFound` a vanished single file would raise.
        let bad = scratch
            .seed_group("cache-atomic-bad")
            .await
            .expect("seed bad group");
        let bad_entry = scratch
            .groups()
            .get(&bad.group_id)
            .await
            .expect("bad entry present");
        std::fs::remove_dir_all(&bad_entry.handle.locator).expect("corrupt bad group repo");

        let failure = rebuild_full(&pool, scratch.backend(), scratch.groups()).await;
        assert!(
            failure.is_err(),
            "rebuild_full must surface the mid-rebuild git failure"
        );

        // The whole attempt (delete + partial repopulate) must have
        // rolled back: the previous known-good row and built flag
        // are exactly as the initial successful build left them,
        // never truncated and never left disagreeing with each other.
        assert!(
            super::super::schema::is_built(&pool)
                .await
                .expect("is_built after failed rebuild")
        );
        let row_count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM indexed_memory")
            .fetch_one(&pool)
            .await
            .expect("count rows after failed rebuild");
        assert_eq!(row_count.0, 1);
    }
}
