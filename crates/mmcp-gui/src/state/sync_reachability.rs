//! Orthogonal "is the sync server reachable right now?" signal.
//!
//! Kept separate from [`crate::state::sync_status::SyncStatus`] so
//! the op-state machine (idle / syncing / last-ok / last-err) and
//! the connectivity signal don't have to share a combinatorial
//! explosion of variants. The background worker spawns a
//! periodic probe that pushes `HealthChanged` outcomes; the UI
//! stores the latest signal here.

#[derive(Debug, Clone, Default)]
pub enum SyncReachability {
    /// No probe has landed yet — early frames after startup. Kept
    /// distinct from `Offline` so the UI can render a neutral
    /// indicator instead of a red "offline" until the probe has
    /// actually spoken.
    #[default]
    Unknown,
    /// Last probe reached the server's manifest endpoint.
    Online,
    /// Last probe failed at the transport layer. Carrying the
    /// reason lets the toolbar tooltip explain WHY the buttons are
    /// disabled (e.g. "connection refused", "dns: no such host").
    Offline { reason: String },
}

impl SyncReachability {
    pub fn is_online(&self) -> bool {
        matches!(self, SyncReachability::Online)
    }
}
