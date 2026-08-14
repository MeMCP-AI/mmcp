//! Local content/semantic cache for mmcp memories.
//!
//! SQLite-backed local index of memory content and feature/milestone data across every group,
//! so a search or rollup query is a single local table scan instead of a live walk of every
//! group's git clone under `~/.mmcp/repos`.
//!
//! ## Storage choice
//!
//! `sqlx` direct over `sea-orm`'s entity/migration machinery: the cache is a fully-rebuildable
//! derived index (`CREATE TABLE IF NOT EXISTS`, freely `DELETE FROM` and repopulate, see
//! [`index::rebuild_full`]), so it needs no real migration chain, and both resolve to the same
//! bundled `libsqlite3-sys` already in the workspace dependency graph.
//!
//! ## Location
//!
//! `<mmcp-home>/cache/index.sqlite3`, a sibling of `repos/` and `sessions/` (see
//! [`crate::home::MmcpHome`]). Local-machine state only, never committed to a group's git repo.
//!
//! ## Update triggers
//!
//! - Local write: see also: crate::memory::write_file_at_path calls [`notify_write`] right
//!   after each git commit. Best-effort: a cache-write failure never fails the underlying
//!   memory write, it only logs, since the cache is a derived artifact the next rebuild repairs.
//! - Server pull/sync: wired at `mmcp-client`'s CLI (`mmcp pull`/`mmcp sync`) and MCP
//!   (`sync_pull`) call sites, after `SyncEngine::pull` reports which groups advanced.
//!
//! ## Missing-index handling
//!
//! Lazy: [`query::keyword_search`] and [`query::semantic_search`] check [`schema::is_built`]
//! first and call [`index::rebuild_full`] on a never-built cache. `mmcp debug cache-rebuild`
//! forces a full rebuild on demand regardless of build state.
//!
//! ## Semantic search
//!
//! See [`embed`] for the embedding approach.

pub mod embed;
pub mod index;
pub mod query;
pub mod schema;

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use sqlx::sqlite::{SqliteConnectOptions, SqlitePool, SqlitePoolOptions};
use thiserror::Error;
use uuid::Uuid;

use mmcp_core::memory::{FeatureStatusParseError, MemoryFile};

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

    /// A `kind = 'feature'` row's stored status string does not
    /// parse as a [`mmcp_core::memory::FeatureStatus`]. Surfaced
    /// instead of silently excluded from a rollup fold (see
    /// `mmcp_store::milestones::rollup::compute`), so a milestone
    /// never reports `Completed` while a real, merely un-migrated
    /// `Blocked` feature is invisible to the count.
    #[error("feature status {raw:?} in the local content cache does not parse: {source}")]
    UnparseableFeatureStatus {
        raw: String,
        #[source]
        source: FeatureStatusParseError,
    },
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
    /// Wire-form status string, present only when `kind == "feature"`.
    /// Lifted out of `FeatureMetadata::status` so
    /// `mmcp_store::milestones::rollup` can fold it without
    /// re-parsing frontmatter.
    pub status: Option<String>,
    /// UUID of the milestone this feature points at, present only
    /// when `kind == "feature"` and `FeatureMetadata::milestone` is
    /// set. The milestone may live in a different group than this
    /// record's own `group_id` (the row itself is not scope-limited);
    /// `rollup::compute` is what restricts the FOLD to the milestone's
    /// own group, see its module doc for the narrowed D3 history.
    pub milestone: Option<Uuid>,
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

/// Maximum number of pooled SQLite connections held open against the
/// cache database. A single local-machine cache under light,
/// same-process concurrency (a handful of MCP tool handlers plus the
/// write/pull hooks) does not need a large pool; this stays small on
/// purpose to bound the number of open file handles and WAL readers.
const CACHE_POOL_MAX_CONNECTIONS: u32 = 4;

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
/// (the CLI, the MCP server) instead go through [`init_from_home`]
/// so every write/pull hook in the process shares one pool.
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
        .max_connections(CACHE_POOL_MAX_CONNECTIONS)
        .connect_with(options)
        .await
        .map_err(|source| CacheError::Open {
            path: path.to_path_buf(),
            source,
        })?;
    schema::ensure_schema(&pool).await?;
    Ok(pool)
}

/// Process-global active cache pool, set once by each consumer's
/// startup path via [`init_from_home`] (the CLI's `main` and the
/// MCP server's `ClientState::initialize_from` both call it before
/// dispatching to any command). [`notify_write`] reads it back to
/// decide whether the write-trigger hook has anything to do.
///
/// A `OnceLock`, the same "set once per process" shape `log` and
/// `tracing` use for their global sink, is the right tool here: a
/// real `mmcp` invocation is one process with exactly one home, so
/// there is never a legitimate reason to swap the active pool mid
/// process. Tests that need to exercise the hook call
/// [`init_from_home`] themselves against a scratch home; because the
/// slot is process-global, such tests must run in their own test
/// binary (a dedicated `tests/*.rs` integration file, which cargo
/// already compiles as its own process) rather than inside this
/// crate's shared unit-test binary, so they cannot race another
/// test's `init_from_home` call for the same slot.
static ACTIVE_POOL: OnceLock<SqlitePool> = OnceLock::new();

/// Initialise the process-global active pool from `home`'s default
/// cache path (see [`default_db_path`]). Idempotent: a second call
/// in the same process is a no-op that keeps the pool the first
/// call installed, matching the "one home per process" invariant.
pub async fn init_from_home(home: &MmcpHome) -> Result<(), CacheError> {
    if ACTIVE_POOL.get().is_some() {
        return Ok(());
    }
    let pool = open_pool(&default_db_path(home)).await?;
    // Benign race: if another task won between the `get()` check
    // above and this `set`, our freshly-opened pool is simply
    // dropped (closes cleanly) and every caller ends up sharing the
    // winner's pool either way.
    let _ = ACTIVE_POOL.set(pool);
    Ok(())
}

/// The active pool, if [`init_from_home`] has run in this process.
/// `None` means no consumer has started the cache subsystem yet
/// (e.g. a unit test exercising unrelated store logic, or a CLI
/// invocation whose command never touches the cache); callers treat
/// that as "the hook is a no-op", never as an error: the cache is
/// a derived artifact, not source-of-truth state, so its absence is
/// never a reason to fail an unrelated operation.
#[must_use]
pub fn active_pool() -> Option<SqlitePool> {
    ACTIVE_POOL.get().cloned()
}

/// Write-trigger hook: called by
/// [`crate::memory::write_file_at_path`] right after a memory write
/// commits. Best-effort: parses `rendered` and upserts it into the
/// active cache pool; any failure (no active pool, unparseable
/// content, a query error) is swallowed after a `tracing::warn!`
/// because the cache is a derived artifact that the next lazy build
/// or debug rebuild repairs, and a cache hiccup must never fail the
/// underlying memory write it is only observing.
pub async fn notify_write(
    group_id: Uuid,
    id: Uuid,
    slug: &str,
    path: &str,
    commit_id: &str,
    rendered: &str,
) {
    let Some(pool) = active_pool() else {
        return;
    };
    let memory_file = match MemoryFile::parse(rendered) {
        Ok(file) => file,
        Err(err) => {
            tracing::warn!(%group_id, %id, %path, error = %err, "cache write-trigger: failed to parse written memory, skipping index update");
            return;
        }
    };
    let record = index::build_record(group_id, id, slug, path, commit_id, &memory_file);
    if let Err(err) = index::upsert_record(&pool, &record).await {
        tracing::warn!(%group_id, %id, %path, error = %err, "cache write-trigger: failed to upsert index row");
    }
}

/// Pull-trigger hook: called by the CLI (`mmcp pull` / `mmcp sync`)
/// and MCP (`sync_pull`) call sites right after
/// `mmcp_sync::SyncEngine::pull` reports which groups advanced.
/// Best-effort, same rationale as [`notify_write`]: re-indexes
/// exactly `updated_group_ids` via [`rebuild_groups`], and swallows
/// any failure after a `tracing::warn!` rather than failing the
/// sync itself.
pub async fn notify_pull(
    backend: &mmcp_git::NativeBackend,
    groups: &crate::groups::GroupIndex,
    updated_group_ids: &[Uuid],
) {
    let Some(pool) = active_pool() else {
        return;
    };
    if let Err(err) = index::rebuild_groups(&pool, backend, groups, updated_group_ids).await {
        tracing::warn!(error = %err, "cache pull-trigger: failed to re-index pulled groups");
    }
}

// Flattened re-exports so callers write `cache::rebuild_full(...)`
// / `cache::keyword_search(...)` instead of reaching into the
// submodule that happens to own the implementation.
pub use index::{RebuildStats, build_record, rebuild_full, rebuild_groups, upsert_record};
pub use query::{ensure_built, keyword_search, semantic_search};
