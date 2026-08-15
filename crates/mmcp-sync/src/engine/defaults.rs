//! Default values shared across the sync engine's push/pull/fetch
//! orchestration.

/// Cap on simultaneous per-group git network round trips (push,
/// fetch, and pull's fast-forward step). Bounds simultaneous
/// connections against the remote and the local backend.
pub(super) const MAX_CONCURRENT_GROUP_TRANSFERS: usize = 6;
