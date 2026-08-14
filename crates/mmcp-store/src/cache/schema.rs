//! SQLite schema for the local content cache.
//!
//! The cache is a fully derived, freely rebuildable index over each group's git repository.
//! This module hand-rolls DDL (`CREATE TABLE IF NOT EXISTS`) instead of a migration runner:
//! there is no user data to migrate, only a schema to (re)create.
//! A future schema change: `DROP TABLE` plus a full rebuild (see [`super::index::rebuild_full`]),
//! not a migration chain.

use sqlx::Sqlite;
use sqlx::sqlite::SqlitePool;

use super::CacheError;

const CREATE_INDEXED_MEMORY: &str = r#"
CREATE TABLE IF NOT EXISTS indexed_memory (
    group_id    TEXT NOT NULL,
    id          TEXT NOT NULL,
    slug        TEXT NOT NULL,
    kind        TEXT NOT NULL,
    name        TEXT NOT NULL,
    description TEXT NOT NULL,
    tags        TEXT NOT NULL,
    body        TEXT NOT NULL,
    path        TEXT NOT NULL,
    commit_id   TEXT NOT NULL,
    embedding   BLOB,
    updated_at  TEXT NOT NULL,
    -- Present only on kind = 'feature' rows: the feature's wire-form
    -- status string and the UUID of the milestone it points at (if
    -- any), lifted out of frontmatter so `mmcp_store::milestones::rollup`
    -- can fold a milestone's status from one table scan instead of
    -- re-parsing every feature memory's frontmatter.
    status      TEXT,
    milestone   TEXT,
    PRIMARY KEY (group_id, id)
)
"#;

const CREATE_SLUG_INDEX: &str =
    "CREATE INDEX IF NOT EXISTS idx_indexed_memory_slug ON indexed_memory(group_id, slug)";

const CREATE_KIND_INDEX: &str =
    "CREATE INDEX IF NOT EXISTS idx_indexed_memory_kind ON indexed_memory(kind)";

const CREATE_MILESTONE_INDEX: &str =
    "CREATE INDEX IF NOT EXISTS idx_indexed_memory_milestone ON indexed_memory(milestone)";

const CREATE_CACHE_META: &str = r#"
CREATE TABLE IF NOT EXISTS cache_meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
)
"#;

/// The `cache_meta` key recording the timestamp of the last successful [`super::index::rebuild_full`] run.
/// Its presence (not row count in `indexed_memory`) is exactly the "index built" signal,
/// the lazy-build-on-read path checks via [`is_built`]:
/// a mirror with zero local memories is a legitimately empty, already-built index, not a missing one,
/// and a plain "is the table empty" check would wrongly rebuild it forever.
pub const LAST_FULL_REBUILD_KEY: &str = "last_full_rebuild_at";

/// Create every table and index this module owns if they do not already exist.
/// Safe to call on every pool open: `IF NOT EXISTS` makes it a no-op against an already-initialised database.
///
/// Also runs the one schema migration this module has ever needed:
/// an on-disk `indexed_memory` table missing the `status`/`milestone` columns gets dropped and recreated,
/// with the build-completion flag cleared so the next lazy read repopulates it.
/// The cache is a fully derived, freely rebuildable index,
/// so `DROP TABLE` plus a full rebuild is simpler and safer than an `ALTER TABLE` chain,
/// for a table that never carries irreplaceable data.
pub async fn ensure_schema(pool: &SqlitePool) -> Result<(), CacheError> {
    sqlx::query(CREATE_INDEXED_MEMORY).execute(pool).await?;
    if !has_column(pool, "indexed_memory", "milestone").await? {
        sqlx::query("DROP TABLE indexed_memory")
            .execute(pool)
            .await?;
        sqlx::query(CREATE_CACHE_META).execute(pool).await?;
        sqlx::query("DELETE FROM cache_meta WHERE key = ?")
            .bind(LAST_FULL_REBUILD_KEY)
            .execute(pool)
            .await?;
        sqlx::query(CREATE_INDEXED_MEMORY).execute(pool).await?;
    }
    sqlx::query(CREATE_SLUG_INDEX).execute(pool).await?;
    sqlx::query(CREATE_KIND_INDEX).execute(pool).await?;
    sqlx::query(CREATE_MILESTONE_INDEX).execute(pool).await?;
    sqlx::query(CREATE_CACHE_META).execute(pool).await?;
    Ok(())
}

/// Whether `table` already has a column named `column`, via
/// `PRAGMA table_info`. Used by [`ensure_schema`] to detect a
/// pre-migration `indexed_memory` table without hand-parsing SQLite
/// error text.
async fn has_column(pool: &SqlitePool, table: &str, column: &str) -> Result<bool, CacheError> {
    // The table-valued-function form of `PRAGMA table_info` projects just the `name` column,
    // so the result binds cleanly onto a one-column `(String,)` row.
    let rows: Vec<(String,)> = sqlx::query_as("SELECT name FROM pragma_table_info(?)")
        .bind(table)
        .fetch_all(pool)
        .await?;
    Ok(rows.iter().any(|(name,)| name == column))
}

/// Whether the index has ever completed a full rebuild, per [`LAST_FULL_REBUILD_KEY`].
pub async fn is_built(pool: &SqlitePool) -> Result<bool, CacheError> {
    let row: Option<(String,)> = sqlx::query_as("SELECT value FROM cache_meta WHERE key = ?")
        .bind(LAST_FULL_REBUILD_KEY)
        .fetch_optional(pool)
        .await?;
    Ok(row.is_some())
}

/// Stamp [`LAST_FULL_REBUILD_KEY`] with `at` (an RFC 3339
/// timestamp), marking the index as built.
///
/// Generic over the executor so callers running a rebuild inside a transaction
/// (see [`super::index::rebuild_full`]) can stamp the flag as part of the SAME transaction,
/// instead of a second, separately-committed statement:
/// the flag must never observably flip to "built" ahead of (or independent from) the row data it describes.
pub async fn mark_built<'e, E>(executor: E, at: &str) -> Result<(), CacheError>
where
    E: sqlx::Executor<'e, Database = Sqlite>,
{
    sqlx::query(
        "INSERT INTO cache_meta (key, value) VALUES (?, ?) \
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
    )
    .bind(LAST_FULL_REBUILD_KEY)
    .bind(at)
    .execute(executor)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn fresh_pool_is_not_built_until_marked() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let db_path = tmp.path().join("index.sqlite3");
        let pool = super::super::open_pool(&db_path).await.expect("open pool");
        assert!(!is_built(&pool).await.expect("is_built"));
        mark_built(&pool, "2026-08-06T00:00:00Z")
            .await
            .expect("mark_built");
        assert!(is_built(&pool).await.expect("is_built"));
    }
}
