//! Default values used across `mmcp-store`.

/// Cap on simultaneous `read_manifest` calls while [`super::groups::GroupIndex::build`]
/// scans `repos_root`. Each candidate directory's manifest read is
/// independent; bounding concurrency here avoids opening every local
/// group repo's bare git handle at once on a mirror with hundreds of
/// groups, while still running the scan far faster than one
/// directory after another.
pub(crate) const MAX_CONCURRENT_MANIFEST_SCANS: usize = 8;

/// Minimum [`super::lock::LockScope`] registry size before its
/// [`mmcp_core::lock_registry::PrunePolicy::Threshold`] sweeps idle
/// entries. Below this size, an O(n) scan on every lock acquisition
/// costs more than the memory it would reclaim; the registry only
/// needs pruning once it has actually grown large.
pub(crate) const LOCK_REGISTRY_PRUNE_THRESHOLD: usize = 64;
