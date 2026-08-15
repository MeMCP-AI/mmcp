//! Default values used across `mmcp-store`.

/// Cap on simultaneous `read_manifest` calls while [`super::groups::GroupIndex::build`]
/// scans `repos_root`. Each candidate directory's manifest read is
/// independent; bounding concurrency here avoids opening every local
/// group repo's bare git handle at once on a mirror with hundreds of
/// groups, while still running the scan far faster than one
/// directory after another.
pub(crate) const MAX_CONCURRENT_MANIFEST_SCANS: usize = 8;
