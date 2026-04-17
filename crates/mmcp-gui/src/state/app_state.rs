//! Aggregate UI state held by the top-level `MmcpGuiApp`.
//!
//! Everything the three panes need to render is here; per-pane
//! transient state (scroll positions, hover tracking) stays inside
//! the pane modules via `egui::Id`. The app state is mutated in two
//! places only: the UI handlers (selection changes) and the
//! outcome-drain in [`MmcpGuiApp::ui`] (after a background task
//! reports back).

use std::collections::HashMap;

use mmcp_core::id::GroupId;
use mmcp_store::GroupEntry;

use crate::state::selection::Selection;
use crate::state::viewer_cache::ViewerCache;

#[derive(Default)]
pub struct AppState {
    /// Snapshot of every locally-mirrored group. Refreshed by
    /// the background worker on startup and on explicit refresh.
    pub groups: Vec<GroupEntry>,

    /// Current user selection across the three panes.
    pub selection: Selection,

    /// Parsed memory bodies indexed by `(group, slug)`. Populated
    /// when the worker reports `TaskOutcome::MemoryLoaded`.
    pub viewer: ViewerCache,

    /// Slugs currently on disk for each group, indexed by group id.
    /// Populated from `list_tree` calls off the worker.
    pub memory_slugs: HashMap<GroupId, Vec<String>>,

    /// Last error string emitted by the background worker. Phase 2
    /// renders this inline below the viewer; Phase 3+ promotes it to
    /// a toast queue.
    pub last_error: Option<String>,
}
