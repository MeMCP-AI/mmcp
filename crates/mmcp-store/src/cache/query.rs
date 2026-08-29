//! Query the local content cache.

use sqlx::sqlite::SqlitePool;
use uuid::Uuid;

use mmcp_git::NativeBackend;

use crate::groups::GroupIndex;

use super::{CacheError, SearchHit};

/// The `ESCAPE` character every `LIKE` pattern in this module uses,
/// so a literal `%` or `_` in a caller-supplied query string matches
/// only itself instead of acting as a `LIKE` wildcard. Escaped by
/// [`like_pattern`] before interpolation; every query string built
/// here carries the matching `ESCAPE '\'` clause.
const LIKE_ESCAPE_CHAR: char = '\\';

/// Build a `LIKE` pattern that matches `needle` as a literal
/// substring: `%` and `_` (`LIKE` wildcards) and the escape
/// character itself are each prefixed with [`LIKE_ESCAPE_CHAR`]
/// before the surrounding `%...%` wrapper is added. Without this, a
/// query containing a literal `%` or `_` over-matches (a bare `%`
/// alone would return the entire index), the defect this module
/// previously shipped.
fn like_pattern(needle: &str) -> String {
    let mut escaped = String::with_capacity(needle.len());
    for c in needle.chars() {
        if c == LIKE_ESCAPE_CHAR || c == '%' || c == '_' {
            escaped.push(LIKE_ESCAPE_CHAR);
        }
        escaped.push(c);
    }
    format!("%{escaped}%")
}

/// Substring/keyword lookup across name, description, tags, slug, and body.
/// Case-insensitive (SQLite `LIKE` is ASCII case-insensitive by default),
/// sufficient for the short English-heavy identifiers and prose this cache indexes.
///
/// Lazy-build-on-read: if the index has never completed a full build (see [`super::schema::is_built`]),
/// this transparently runs [`super::index::rebuild_full`] first,
/// so the caller never has to know or care whether the cache was cold.
pub async fn keyword_search(
    pool: &SqlitePool,
    backend: &NativeBackend,
    groups: &GroupIndex,
    query: &str,
    limit: u32,
) -> Result<Vec<SearchHit>, CacheError> {
    ensure_built(pool, backend, groups).await?;

    let pattern = like_pattern(query);
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT group_id, id, slug, kind, name, description, path FROM indexed_memory \
         WHERE name LIKE ?1 ESCAPE '\\' OR description LIKE ?1 ESCAPE '\\' \
            OR tags LIKE ?1 ESCAPE '\\' OR slug LIKE ?1 ESCAPE '\\' \
            OR body LIKE ?1 ESCAPE '\\' \
         ORDER BY updated_at DESC LIMIT ?2",
    )
    .bind(&pattern)
    .bind(i64::from(limit))
    .fetch_all(pool)
    .await?;

    rows.into_iter().map(Row::into_hit).collect()
}

/// Slug/name-only substring lookup: the restricted match surface
/// `mmcp-client`'s `search_memories` tool requires, unlike
/// [`keyword_search`]'s broader name/description/tags/slug/body scan.
/// A prefilter, not the authoritative matcher: `LIKE`'s ASCII-only
/// case folding can only MISS a row a full Unicode-aware
/// case-insensitive comparison would match (never over-match, since
/// every returned row genuinely contains the escaped literal
/// substring), so a caller needing exact substring semantics
/// re-checks `slug`/`name` on the returned rows rather than trusting
/// this as a final answer for non-ASCII queries.
///
/// Returns every match with no `LIMIT`: capping to a caller-facing
/// page size is the caller's job once results from several needles
/// (or several group/scope filters) are merged; capping here would
/// let the SQL-side cutoff silently starve a later merge step of
/// candidates that belong in the final page. [`semantic_search`]
/// already fetches its whole candidate set the same way.
///
/// Lazy-build-on-read, same as [`keyword_search`].
pub async fn search_slug_name(
    pool: &SqlitePool,
    backend: &NativeBackend,
    groups: &GroupIndex,
    needle: &str,
) -> Result<Vec<SearchHit>, CacheError> {
    ensure_built(pool, backend, groups).await?;

    let pattern = like_pattern(needle);
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT group_id, id, slug, kind, name, description, path FROM indexed_memory \
         WHERE slug LIKE ?1 ESCAPE '\\' OR name LIKE ?1 ESCAPE '\\' \
         ORDER BY updated_at DESC",
    )
    .bind(&pattern)
    .fetch_all(pool)
    .await?;

    rows.into_iter().map(Row::into_hit).collect()
}

/// Semantic-similarity lookup: embed `query` (see [`super::embed`]),
/// and rank every indexed memory by cosine similarity against its stored embedding,
/// returning the top `limit` matches with their score.
/// A brute-force scan over every row, not an approximate-nearest-neighbour index,
/// appropriate at the "modest local dataset" scale this cache targets.
/// See the module docs on [`super::embed`] for why an ANN index is deliberately not used here.
///
/// Lazy-build-on-read, same as [`keyword_search`].
pub async fn semantic_search(
    pool: &SqlitePool,
    backend: &NativeBackend,
    groups: &GroupIndex,
    query: &str,
    limit: u32,
) -> Result<Vec<SearchHit>, CacheError> {
    ensure_built(pool, backend, groups).await?;

    let query_embedding = super::embed::embed_text(query);
    let query_norm = super::embed::norm(&query_embedding);
    let rows: Vec<EmbeddingRow> = sqlx::query_as(
        "SELECT group_id, id, slug, kind, name, description, path, embedding FROM indexed_memory",
    )
    .fetch_all(pool)
    .await?;

    let mut scored: Vec<(f32, Row)> = rows
        .into_iter()
        .filter_map(|row| {
            let embedding = row
                .embedding
                .as_deref()
                .map(super::embed::bytes_to_vector)?;
            let score = super::embed::cosine_similarity_with_query_norm(
                &query_embedding,
                query_norm,
                &embedding,
            );
            Some((
                score,
                Row {
                    group_id: row.group_id,
                    id: row.id,
                    slug: row.slug,
                    kind: row.kind,
                    name: row.name,
                    description: row.description,
                    path: row.path,
                },
            ))
        })
        .collect();

    // Descending by score; `total_cmp` handles the all-`f32` sort
    // key correctly (including the `NaN`-from-empty-vector edge
    // case `cosine_similarity` never actually produces, since it
    // returns `0.0` rather than dividing by zero).
    //
    // Top-`limit` selection, not a full sort: `select_nth_unstable_by`
    // partitions in O(n) instead of O(n log n), then only the
    // selected prefix is sorted so its relative ORDER matches a full
    // sort exactly; the discarded tail's order is unspecified, which
    // callers never observe since it is never returned.
    let limit = limit as usize;
    if limit == 0 {
        return Ok(Vec::new());
    }
    if limit < scored.len() {
        let pivot = limit - 1;
        scored.select_nth_unstable_by(pivot, |a, b| b.0.total_cmp(&a.0));
        scored.truncate(limit);
    }
    scored.sort_by(|a, b| b.0.total_cmp(&a.0));

    scored
        .into_iter()
        .map(|(score, row)| {
            row.into_hit().map(|mut hit| {
                hit.score = Some(score);
                hit
            })
        })
        .collect()
}

/// Run [`super::index::rebuild_full`] iff the index has never completed a build.
/// Returns `true` when a rebuild actually ran.
/// The shared lazy-build primitive every cache query entry point calls before touching `indexed_memory`.
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

/// Same shape as [`Row`] plus the raw embedding BLOB, used only by
/// [`semantic_search`]'s scan (the plain [`keyword_search`] query
/// never needs to decode an embedding, so it stays on the lighter
/// [`Row`] projection).
#[derive(sqlx::FromRow)]
struct EmbeddingRow {
    group_id: String,
    id: String,
    slug: String,
    kind: String,
    name: String,
    description: String,
    path: String,
    embedding: Option<Vec<u8>>,
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
    #![allow(clippy::unwrap_used, clippy::expect_used)]
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
        write_named_sample(scratch, handle, slug, slug, body).await;
    }

    /// Same as [`write_sample`] but with a `name` distinct from
    /// `slug`, so a test can put an arbitrary substring (including a
    /// `LIKE` metacharacter that on-disk slug/path conventions would
    /// reject) into the frontmatter `name` field without touching
    /// the file's path.
    async fn write_named_sample(
        scratch: &ScratchHome,
        handle: &mmcp_git::RepoHandle,
        slug: &str,
        name: &str,
        body: &str,
    ) {
        let source = format!(
            "+++\nname = \"{name}\"\ndescription = \"about {slug}\"\nkind = \"scratch\"\n+++\n\n{body}\n"
        );
        let file = MemoryFile::parse(&source).expect("parse");
        let rendered = file.to_string().expect("render");
        let id = Uuid::now_v7();
        let path =
            mmcp_core::conventions::memory_path(slug, mmcp_core::id::MemoryId::from_uuid(id));
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

    #[tokio::test]
    async fn semantic_search_ranks_the_lexically_closer_memory_first() {
        let scratch = ScratchHome::new().await.expect("scratch home");
        let seeded = scratch
            .seed_group("cache-semantic-test")
            .await
            .expect("seed group");
        let entry = scratch.groups().get(&seeded.group_id).await.expect("entry");

        write_sample(
            &scratch,
            &entry.handle,
            "quokka-notes",
            "the quokka is a small marsupial native to Australia",
        )
        .await;
        write_sample(
            &scratch,
            &entry.handle,
            "finance-notes",
            "quarterly earnings and the stock market closed higher today",
        )
        .await;

        let tmp = tempfile::TempDir::new().expect("tempdir");
        let pool = super::super::open_pool(&tmp.path().join("index.sqlite3"))
            .await
            .expect("open pool");

        // A query that never contains the literal word "quokka" still ranks the marsupial memory first
        // because it shares more vocabulary with the query than the finance memory does.
        // This is what distinguishes semantic_search from a plain keyword LIKE scan.
        let hits = semantic_search(
            &pool,
            scratch.backend(),
            scratch.groups(),
            "marsupials found in Australia",
            10,
        )
        .await
        .expect("semantic_search");

        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].slug, "quokka-notes");
        let score = hits[0].score.expect("semantic hit carries a score");
        assert!(score > hits[1].score.expect("second hit also scored"));
    }

    /// `limit` strictly smaller than the row count exercises the
    /// `select_nth_unstable_by` top-k path rather than the full-sort path.
    /// Only the SELECTED prefix's order is asserted,
    /// per the top-k contract: the discarded tail's internal order is
    /// unspecified.
    #[tokio::test]
    async fn semantic_search_top_k_selection_preserves_rank_order() {
        let scratch = ScratchHome::new().await.expect("scratch home");
        let seeded = scratch
            .seed_group("cache-topk-test")
            .await
            .expect("seed group");
        let entry = scratch.groups().get(&seeded.group_id).await.expect("entry");

        write_sample(
            &scratch,
            &entry.handle,
            "quokka-notes",
            "the quokka is a small marsupial native to Australia",
        )
        .await;
        write_sample(
            &scratch,
            &entry.handle,
            "koala-notes",
            "the koala is a marsupial that lives in Australia eating eucalyptus",
        )
        .await;
        write_sample(
            &scratch,
            &entry.handle,
            "finance-notes",
            "quarterly earnings and the stock market closed higher today",
        )
        .await;

        let tmp = tempfile::TempDir::new().expect("tempdir");
        let pool = super::super::open_pool(&tmp.path().join("index.sqlite3"))
            .await
            .expect("open pool");

        // Three rows exist; ask for the top 2 so the selection path
        // (limit < row count) runs instead of the full-sort path.
        let hits = semantic_search(
            &pool,
            scratch.backend(),
            scratch.groups(),
            "marsupials found in Australia",
            2,
        )
        .await
        .expect("semantic_search");

        assert_eq!(hits.len(), 2, "top-k must return exactly `limit` hits");
        // Both marsupial memories must outrank the unrelated finance
        // memory; their own relative order between the two is what
        // this test pins (quokka/koala are near-symmetric in shared
        // vocabulary, so the assertion focuses on both outranking the
        // finance memory rather than asserting a specific 1st/2nd tie
        // break the hashing-trick scoring is not obligated to keep
        // stable).
        let slugs: Vec<&str> = hits.iter().map(|h| h.slug.as_str()).collect();
        assert!(slugs.contains(&"quokka-notes"));
        assert!(slugs.contains(&"koala-notes"));
        assert!(!slugs.contains(&"finance-notes"));
        // The returned prefix stays sorted by descending score.
        let first_score = hits[0].score.expect("first hit scored");
        let second_score = hits[1].score.expect("second hit scored");
        assert!(first_score >= second_score);
    }

    /// A literal `%` in the query must match only rows genuinely
    /// containing that character, never act as a `LIKE` wildcard
    /// that also pulls in an unrelated sibling.
    #[tokio::test]
    async fn keyword_search_escapes_literal_percent() {
        let scratch = ScratchHome::new().await.expect("scratch home");
        let seeded = scratch
            .seed_group("cache-escape-percent-test")
            .await
            .expect("seed group");
        let entry = scratch.groups().get(&seeded.group_id).await.expect("entry");

        write_named_sample(
            &scratch,
            &entry.handle,
            "percent-rule",
            "100% rule",
            "always applies",
        )
        .await;
        write_named_sample(
            &scratch,
            &entry.handle,
            "percentx-rule",
            "100x rule",
            "never applies",
        )
        .await;

        let tmp = tempfile::TempDir::new().expect("tempdir");
        let pool = super::super::open_pool(&tmp.path().join("index.sqlite3"))
            .await
            .expect("open pool");

        let hits = keyword_search(&pool, scratch.backend(), scratch.groups(), "100%", 10)
            .await
            .expect("keyword_search");
        assert_eq!(
            hits.len(),
            1,
            "a literal '%' must not wildcard-match '100x rule'; got: {hits:?}"
        );
        assert_eq!(hits[0].slug, "percent-rule");
    }

    /// A literal `_` in the query must match only rows genuinely
    /// containing that character, never `LIKE`'s single-character
    /// wildcard behavior.
    #[tokio::test]
    async fn keyword_search_escapes_literal_underscore() {
        let scratch = ScratchHome::new().await.expect("scratch home");
        let seeded = scratch
            .seed_group("cache-escape-underscore-test")
            .await
            .expect("seed group");
        let entry = scratch.groups().get(&seeded.group_id).await.expect("entry");

        write_named_sample(
            &scratch,
            &entry.handle,
            "snake-case-rule",
            "max_value setting",
            "the real setting",
        )
        .await;
        write_named_sample(
            &scratch,
            &entry.handle,
            "camel-case-rule",
            "maxAvalue setting",
            "a decoy `_` would wildcard-match this",
        )
        .await;

        let tmp = tempfile::TempDir::new().expect("tempdir");
        let pool = super::super::open_pool(&tmp.path().join("index.sqlite3"))
            .await
            .expect("open pool");

        let hits = keyword_search(&pool, scratch.backend(), scratch.groups(), "max_value", 10)
            .await
            .expect("keyword_search");
        assert_eq!(
            hits.len(),
            1,
            "a literal '_' must not wildcard-match 'maxAvalue'; got: {hits:?}"
        );
        assert_eq!(hits[0].slug, "snake-case-rule");
    }

    /// A query of exactly `%` must return only rows whose indexed
    /// text genuinely contains a literal `%`, never the entire
    /// index (the behavior the tracked defect this fix closes
    /// previously produced).
    #[tokio::test]
    async fn keyword_search_bare_percent_query_does_not_return_the_whole_index() {
        let scratch = ScratchHome::new().await.expect("scratch home");
        let seeded = scratch
            .seed_group("cache-escape-bare-percent-test")
            .await
            .expect("seed group");
        let entry = scratch.groups().get(&seeded.group_id).await.expect("entry");

        write_named_sample(
            &scratch,
            &entry.handle,
            "has-percent",
            "100% rule",
            "contains the literal character",
        )
        .await;
        write_named_sample(
            &scratch,
            &entry.handle,
            "no-percent",
            "plain rule",
            "no metacharacter here",
        )
        .await;

        let tmp = tempfile::TempDir::new().expect("tempdir");
        let pool = super::super::open_pool(&tmp.path().join("index.sqlite3"))
            .await
            .expect("open pool");

        let hits = keyword_search(&pool, scratch.backend(), scratch.groups(), "%", 10)
            .await
            .expect("keyword_search");
        assert_eq!(
            hits.len(),
            1,
            "a bare '%' query must not act as a match-everything wildcard; got: {hits:?}"
        );
        assert_eq!(hits[0].slug, "has-percent");
    }

    /// [`search_slug_name`] restricts the match surface to slug and
    /// name: a query hitting only the body must return zero hits,
    /// unlike [`keyword_search`], which also scans body.
    #[tokio::test]
    async fn search_slug_name_ignores_a_body_only_match() {
        let scratch = ScratchHome::new().await.expect("scratch home");
        let seeded = scratch
            .seed_group("cache-slug-name-surface-test")
            .await
            .expect("seed group");
        let entry = scratch.groups().get(&seeded.group_id).await.expect("entry");

        write_named_sample(
            &scratch,
            &entry.handle,
            "unrelated-slug",
            "unrelated name",
            "the quokka is a small marsupial",
        )
        .await;

        let tmp = tempfile::TempDir::new().expect("tempdir");
        let pool = super::super::open_pool(&tmp.path().join("index.sqlite3"))
            .await
            .expect("open pool");

        let hits = search_slug_name(&pool, scratch.backend(), scratch.groups(), "quokka")
            .await
            .expect("search_slug_name");
        assert!(
            hits.is_empty(),
            "a body-only match must not surface through the slug/name-restricted search: {hits:?}"
        );

        // The same needle against slug/name still finds a real match,
        // proving the empty result above is the surface restriction,
        // not a broken query.
        let hits = search_slug_name(&pool, scratch.backend(), scratch.groups(), "unrelated")
            .await
            .expect("search_slug_name");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].slug, "unrelated-slug");
    }

    /// [`search_slug_name`] applies the same `LIKE`-escaping as
    /// [`keyword_search`]: a literal `%` in the needle matches only
    /// the genuine literal, not a wildcard-expanded sibling.
    #[tokio::test]
    async fn search_slug_name_escapes_literal_percent() {
        let scratch = ScratchHome::new().await.expect("scratch home");
        let seeded = scratch
            .seed_group("cache-slug-name-escape-test")
            .await
            .expect("seed group");
        let entry = scratch.groups().get(&seeded.group_id).await.expect("entry");

        write_named_sample(&scratch, &entry.handle, "percent-rule", "100% rule", "body").await;
        write_named_sample(
            &scratch,
            &entry.handle,
            "percentx-rule",
            "100x rule",
            "body",
        )
        .await;

        let tmp = tempfile::TempDir::new().expect("tempdir");
        let pool = super::super::open_pool(&tmp.path().join("index.sqlite3"))
            .await
            .expect("open pool");

        let hits = search_slug_name(&pool, scratch.backend(), scratch.groups(), "100%")
            .await
            .expect("search_slug_name");
        assert_eq!(
            hits.len(),
            1,
            "a literal '%' must not wildcard-match '100x rule'; got: {hits:?}"
        );
        assert_eq!(hits[0].slug, "percent-rule");
    }
}
