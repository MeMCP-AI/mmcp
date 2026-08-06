//! Populate the cache: full rebuilds and single-record upserts.

use jiff::Timestamp;
use mmcp_core::memory::MemoryFile;
use mmcp_git::{GitBackend, NativeBackend, Rev};
use sqlx::sqlite::SqlitePool;
use uuid::Uuid;

use crate::groups::GroupIndex;
use crate::memory::list_all_memory_files;

use super::{CacheError, IndexedRecord};

/// Outcome of a [`rebuild_full`] run.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RebuildStats {
    pub groups_scanned: usize,
    pub memories_indexed: usize,
}

/// Rebuild the whole cache from scratch: every memory in every
/// locally-mirrored group, read at each group's current `HEAD`.
/// Existing rows are cleared first so a memory deleted or moved
/// upstream does not linger as a stale hit. This is both the lazy
/// -build-on-read primitive (see [`super::query`]) and the body of
/// the debug/admin "force rebuild" entry point.
pub async fn rebuild_full(
    pool: &SqlitePool,
    backend: &NativeBackend,
    groups: &GroupIndex,
) -> Result<RebuildStats, CacheError> {
    let mut stats = RebuildStats::default();
    sqlx::query("DELETE FROM indexed_memory")
        .execute(pool)
        .await?;

    for entry in groups.list().await {
        stats.groups_scanned += 1;
        let rev = Rev::head();
        let files = list_all_memory_files(backend, &entry.handle, &rev).await?;
        for file_ref in files {
            let bytes = match backend.read_file(&entry.handle, &file_ref.path, &rev).await {
                Ok(bytes) => bytes,
                // Raced with a concurrent delete/move between the
                // listing and the read: skip rather than fail the
                // whole rebuild over one vanished file.
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
            upsert_record(pool, &record).await?;
            stats.memories_indexed += 1;
        }
    }

    let now = Timestamp::now().to_string();
    super::schema::mark_built(pool, &now).await?;
    Ok(stats)
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
    }
}

/// Insert or replace one memory's row.
pub async fn upsert_record(pool: &SqlitePool, record: &IndexedRecord) -> Result<(), CacheError> {
    let tags_json = serde_json::to_string(&record.tags).unwrap_or_default();
    let now = Timestamp::now().to_string();
    sqlx::query(
        "INSERT INTO indexed_memory \
           (group_id, id, slug, kind, name, description, tags, body, path, commit_id, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
         ON CONFLICT(group_id, id) DO UPDATE SET \
           slug = excluded.slug, kind = excluded.kind, name = excluded.name, \
           description = excluded.description, tags = excluded.tags, body = excluded.body, \
           path = excluded.path, commit_id = excluded.commit_id, updated_at = excluded.updated_at",
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
    .bind(now)
    .execute(pool)
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
            let id = uuid::Uuid::now_v7();
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
}
