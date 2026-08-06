//! Local content/semantic cache for mmcp memories.
//!
//! Today mmcp has no local index of memory content beyond the git
//! clones under `~/.mmcp/repos` themselves: every read walks a
//! bare repo tree and parses frontmatter on the fly. That is fine
//! for a single group but does not scale to "search across every
//! locally-mirrored memory" or "roll a milestone's status up across
//! groups" without live-walking every group's git repo on every
//! query. This module is a real, local-machine, SQLite-backed cache
//! that indexes memory content — not just feature/milestone data —
//! across every group so those queries become a single local table
//! scan instead of N git tree walks.
//!
//! ## Storage choice
//!
//! SQLite via `sqlx` directly (not through `sea-orm`'s entity /
//! migration machinery): the cache is a small, fully-owned, freely
//! rebuildable derived index — a couple of tables this crate
//! creates with `CREATE TABLE IF NOT EXISTS` and can safely
//! `DELETE FROM` + repopulate at will (see [`index::rebuild_full`])
//! — so there is no user data that ever needs a real migration
//! chain, and pulling in `sea-orm-migration`'s versioned runner for
//! that would be ceremony without benefit. Using `sqlx` directly
//! still resolves to the exact same underlying SQLite build
//! (bundled `libsqlite3-sys`) `sea-orm` already pulls in elsewhere
//! in this workspace, so no second copy of the C library enters the
//! dependency graph and no new database *technology* is introduced,
//! only a lighter-weight way of talking to the one already in use.
//!
//! ## Location
//!
//! `<mmcp-home>/cache/index.sqlite3` — a sibling of `repos/` and
//! `sessions/` under the existing `~/.mmcp` layout (see
//! [`crate::home::MmcpHome`]). This is local-machine state, never
//! committed to any group's git repository; callers that write to
//! `~/.mmcp` are expected to gitignore or otherwise exclude the
//! `cache/` subdirectory the same way `repos/` already is.
//!
//! ## Update triggers
//!
//! - **Local write**: [`crate::memory::write_file_at_path`] — the
//!   single choke point every memory write with rendered content
//!   (create, update, `edit_memory_body`, feature/issue create and
//!   update, archive import) commits through — calls a
//!   `notify_write` hook right after the git commit lands (landing
//!   in a follow-up commit alongside the rest of the write-trigger
//!   wiring). Best-effort:
//!   a cache-write failure never fails the underlying memory write,
//!   it only gets logged, since the cache is a derived artifact and
//!   the next lazy-build or debug rebuild repairs it.
//! - **Server pull/sync**: wired at the CLI (`mmcp pull` / `mmcp
//!   sync`) and MCP (`sync_pull`) call sites in `mmcp-client`, right
//!   after `SyncEngine::pull` reports which groups advanced. See
//!   that crate's `commands/sync.rs` and `commands/serve.rs`.
//!
//! ## Missing-index handling
//!
//! Lazy: [`query::keyword_search`] and [`query::semantic_search`]
//! both check [`schema::is_built`] first and transparently call
//! [`index::rebuild_full`] when the cache has never completed a
//! build, so the first query against a fresh mirror pays the
//! rebuild cost and every query after that is a plain lookup. A
//! debug/admin entry point (`mmcp debug cache-rebuild`, see
//! `mmcp-client`) also forces a full rebuild on demand regardless
//! of whether the index already looks built.
//!
//! ## Semantic search
//!
//! Lands in a follow-up commit alongside its own `embed` module,
//! which documents the embedding approach and its rationale.

pub mod index;
pub mod query;
pub mod schema;

use std::path::{Path, PathBuf};

use sqlx::sqlite::{SqliteConnectOptions, SqlitePool, SqlitePoolOptions};
use thiserror::Error;
use uuid::Uuid;

use crate::home::MmcpHome;

/// Every failure this module can surface.
#[derive(Debug, Error)]
pub enum CacheError {
    /// Opening or creating the SQLite database file failed.
    #[error("opening cache database at {path}: {source}")]
    Open {
        path: PathBuf,
        #[source]
        source: sqlx::Error,
    },

    /// A schema statement or query against an already-open pool
    /// failed.
    #[error("cache database query failed: {0}")]
    Query(#[from] sqlx::Error),

    /// Walking a group's memory tree to rebuild the index failed.
    #[error("walking group repository: {0}")]
    Git(#[from] mmcp_git::GitError),
}

/// One row of the local content cache: everything about a memory
/// needed to answer a keyword or semantic query without touching
/// git again. Built by [`index::build_record`] from a parsed
/// [`mmcp_core::memory::MemoryFile`] plus the addressing metadata
/// the caller already resolved.
#[derive(Debug, Clone)]
pub struct IndexedRecord {
    pub group_id: Uuid,
    pub id: Uuid,
    pub slug: String,
    pub kind: String,
    pub name: String,
    pub description: String,
    pub tags: Vec<String>,
    pub body: String,
    pub path: String,
    pub commit_id: String,
}

/// One hit returned by [`query::keyword_search`] or
/// [`query::semantic_search`]: enough to locate the memory again
/// (`group_id` + `id`, or `group_id` + `slug` + `path`) plus the
/// display fields a search UX needs without a second lookup.
#[derive(Debug, Clone)]
pub struct SearchHit {
    pub group_id: Uuid,
    pub id: Uuid,
    pub slug: String,
    pub kind: String,
    pub name: String,
    pub description: String,
    pub path: String,
    /// Present only for [`query::semantic_search`] results; `None`
    /// for [`query::keyword_search`], which has no similarity score
    /// to report.
    pub score: Option<f32>,
}

const CACHE_SUBDIR: &str = "cache";
const CACHE_DB_FILE: &str = "index.sqlite3";

/// Default on-disk location of the cache database:
/// `<mmcp-home>/cache/index.sqlite3`.
#[must_use]
pub fn default_db_path(home: &MmcpHome) -> PathBuf {
    home.root().join(CACHE_SUBDIR).join(CACHE_DB_FILE)
}

/// Open (creating if absent) the SQLite pool at `path` and ensure
/// the schema exists. Creates parent directories as needed. Callers
/// that only need a throwaway pool for a test or a one-shot rebuild
/// call this directly with an explicit path; long-lived consumers
/// (the CLI, the MCP server) instead go through a process-global
/// active pool (landing in a follow-up commit alongside the
/// write-trigger wiring) so every write/pull hook in the process
/// shares one connection pool.
pub async fn open_pool(path: &Path) -> Result<SqlitePool, CacheError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|source| CacheError::Open {
            path: path.to_path_buf(),
            source: sqlx::Error::Io(source),
        })?;
    }
    let options = SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(true);
    let pool = SqlitePoolOptions::new()
        .max_connections(4)
        .connect_with(options)
        .await
        .map_err(|source| CacheError::Open {
            path: path.to_path_buf(),
            source,
        })?;
    schema::ensure_schema(&pool).await?;
    Ok(pool)
}

// Flattened re-exports so callers write `cache::rebuild_full(...)`
// / `cache::keyword_search(...)` instead of reaching into the
// submodule that happens to own the implementation.
pub use index::{RebuildStats, build_record, rebuild_full, upsert_record};
pub use query::{ensure_built, keyword_search};
