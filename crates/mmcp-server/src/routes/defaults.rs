//! Default values and constants shared across `routes/` handlers.

/// Cap on simultaneous per-group `read_manifest`/`walk_history` calls
/// while building the `/sync/manifest` response. Each row's git reads
/// are independent; bounding concurrency here avoids opening every
/// group's bare repo at once on a server with hundreds of groups,
/// while still running far faster than one row after another.
pub(crate) const MAX_CONCURRENT_MANIFEST_LOOKUPS: usize = 8;
