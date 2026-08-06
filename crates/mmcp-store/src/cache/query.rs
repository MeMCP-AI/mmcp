//! Query the local content cache.

use sqlx::sqlite::SqlitePool;
use uuid::Uuid;

use mmcp_git::NativeBackend;

use crate::groups::GroupIndex;

use super::{CacheError, SearchHit};

/// Substring/keyword lookup across name, description, tags, slug,
/// and body. Case-insensitive (SQLite `LIKE` is ASCII
/// case-insensitive by default, which is sufficient for the short
/// English-heavy identifiers and prose this cache indexes).
///
/// Lazy-build-on-read: if the index has never completed a full
/// build (see [`super::schema::is_built`]), this transparently runs
/// [`super::index::rebuild_full`] first so the caller never has to
/// know or care whether the cache was cold.
pub async fn keyword_search(
    pool: &SqlitePool,
    backend: &NativeBackend,
    groups: &GroupIndex,
    query: &str,
    limit: u32,
) -> Result<Vec<SearchHit>, CacheError> {
    ensure_built(pool, backend, groups).await?;

    let pattern = format!("%{query}%");
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT group_id, id, slug, kind, name, description, path FROM indexed_memory \
         WHERE name LIKE ?1 OR description LIKE ?1 OR tags LIKE ?1 \
            OR slug LIKE ?1 OR body LIKE ?1 \
         ORDER BY updated_at DESC LIMIT ?2",
    )
    .bind(&pattern)
    .bind(i64::from(limit))
    .fetch_all(pool)
    .await?;

    rows.into_iter().map(Row::into_hit).collect()
}

/// Run [`super::index::rebuild_full`] iff the index has never
/// completed a build. Returns `true` when a rebuild actually ran.
/// The shared lazy-build primitive every cache query entry point
/// calls before touching `indexed_memory`.
pub async fn ensure_built(
    pool: &SqlitePool,
    backend: &NativeBackend,
    groups: &GroupIndex,
) -> Result<bool, CacheError> {
    if super::schema::is_built(pool).await? {
        return Ok(false);
    }
    super::index::rebuild_full(pool, backend, groups).await?;
    Ok(true)
}

#[derive(sqlx::FromRow)]
struct Row {
    group_id: String,
    id: String,
    slug: String,
    kind: String,
    name: String,
    description: String,
    path: String,
}

impl Row {
    fn into_hit(self) -> Result<SearchHit, CacheError> {
        let group_id = Uuid::parse_str(&self.group_id).map_err(invalid_uuid)?;
        let id = Uuid::parse_str(&self.id).map_err(invalid_uuid)?;
        Ok(SearchHit {
            group_id,
            id,
            slug: self.slug,
            kind: self.kind,
            name: self.name,
            description: self.description,
            path: self.path,
            score: None,
        })
    }
}

fn invalid_uuid(source: uuid::Error) -> CacheError {
    // A row with a malformed UUID column can only come from direct
    // manual tampering with the cache file (never from
    // `upsert_record`, which always binds `Uuid::to_string()`), so
    // this maps onto the generic query-failure variant rather than
    // getting its own dedicated error case.
    CacheError::Query(sqlx::Error::Decode(Box::new(source)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::WriteFileOptions;
    use crate::testing::ScratchHome;
    use mmcp_core::memory::MemoryFile;

    async fn write_sample(
        scratch: &ScratchHome,
        handle: &mmcp_git::RepoHandle,
        slug: &str,
        body: &str,
    ) {
        let source = format!(
            "+++\nname = \"{slug}\"\ndescription = \"about {slug}\"\nkind = \"scratch\"\n+++\n\n{body}\n"
        );
        let file = MemoryFile::parse(&source).expect("parse");
        let rendered = file.to_string().expect("render");
        let id = Uuid::now_v7();
        let path = mmcp_core::conventions::memory_path(slug, id);
        crate::memory::write_file_at_path(
            scratch.backend(),
            handle,
            &path,
            &rendered,
            scratch.author(),
            WriteFileOptions::default(),
        )
        .await
        .expect("write sample");
    }

    #[tokio::test]
    async fn keyword_search_lazily_builds_then_finds_a_match() {
        let scratch = ScratchHome::new().await.expect("scratch home");
        let seeded = scratch
            .seed_group("cache-query-test")
            .await
            .expect("seed group");
        let entry = scratch.groups().get(&seeded.group_id).await.expect("entry");

        write_sample(
            &scratch,
            &entry.handle,
            "quokka-notes",
            "the quokka is a small marsupial",
        )
        .await;
        write_sample(
            &scratch,
            &entry.handle,
            "capybara-notes",
            "the capybara is the largest rodent",
        )
        .await;

        let tmp = tempfile::TempDir::new().expect("tempdir");
        let pool = super::super::open_pool(&tmp.path().join("index.sqlite3"))
            .await
            .expect("open pool");

        // No rebuild has run yet: this call must trigger the lazy
        // build itself and still return the right hit.
        assert!(
            !super::super::schema::is_built(&pool)
                .await
                .expect("is_built")
        );
        let hits = keyword_search(&pool, scratch.backend(), scratch.groups(), "quokka", 10)
            .await
            .expect("keyword_search");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].slug, "quokka-notes");
        assert!(
            super::super::schema::is_built(&pool)
                .await
                .expect("is_built")
        );

        // A second call against the now-built index must not
        // re-scan or duplicate rows.
        let hits_again = keyword_search(&pool, scratch.backend(), scratch.groups(), "capybara", 10)
            .await
            .expect("keyword_search again");
        assert_eq!(hits_again.len(), 1);
        assert_eq!(hits_again[0].slug, "capybara-notes");
    }
}
