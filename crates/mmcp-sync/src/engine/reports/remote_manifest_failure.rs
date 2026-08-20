//! [`RemoteManifestFailure`]: one remote's failed manifest poll
//! during `fetch`.

use crate::error::SyncError;

/// One `mmcp-server`-transport remote whose `/sync/manifest` poll
/// itself errored during `fetch`.
///
/// Distinct from [`super::GroupSyncFailure`]: a manifest poll never
/// got far enough to discover any group, so it has no group id to
/// key on, and fabricating one to reuse `GroupSyncFailure` would
/// violate that type's own "one group's failed attempt" contract.
///
/// `fetch` never lets one remote's manifest failure abort another
/// remote's poll (see [`crate::SyncEngine::fetch`]'s doc comment):
/// every OTHER `mmcp-server` remote is still polled, and every group
/// it advertises still lands in the report normally. `pull` carries
/// this list forward unchanged from the `fetch` phase it delegates
/// to, since its own fast-forward phase never produces a new
/// manifest failure.
#[derive(Debug)]
pub struct RemoteManifestFailure {
    /// Name of the [`crate::BoundRemote`] whose manifest poll failed.
    pub remote_name: String,
    /// The underlying failure.
    pub error: SyncError,
}
