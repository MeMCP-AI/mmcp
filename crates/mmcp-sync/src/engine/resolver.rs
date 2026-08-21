//! Local-handle resolution the sync engine needs to turn a group id
//! into something it can actually push/fetch against.

use uuid::Uuid;

use crate::error::IndexContended;

/// Resolves a `group_id` to its local [`mmcp_git::RepoHandle`] and
/// enumerates every locally-known group.
///
/// The client's `GroupIndex` implements this naturally; the sync
/// engine stays decoupled from any specific index type so tests
/// can supply a tiny in-memory resolver.
///
/// `Send + Sync` are required so the engine's async methods can be
/// spawned onto a multi-threaded runtime (e.g. the MCP tool router
/// boxes returned futures with a `Send` bound). Existing resolver
/// impls in this workspace are already thread-safe; the bound
/// simply makes that requirement explicit.
pub trait GroupHandleResolver: Send + Sync {
    /// Look up the local bare-repo handle for a group.
    ///
    /// `Ok(Some(handle))`: the group is indexed locally. `Ok(None)`:
    /// the group is genuinely not indexed (never seen, or removed).
    /// `Err(IndexContended)`: the backing index could not be read
    /// right now without blocking; transient, expected to clear on
    /// the caller's next attempt.
    fn resolve(&self, group_id: Uuid) -> Result<Option<mmcp_git::RepoHandle>, IndexContended>;
    /// Snapshot every locally-indexed group id. Used by `push`
    /// to iterate local groups without a manifest round trip.
    /// Returns an empty vector when the underlying index cannot
    /// be read without blocking, which the engine treats as
    /// "no groups to push" - the caller retries on its next tick.
    fn iter_group_ids(&self) -> Vec<Uuid>;
}
