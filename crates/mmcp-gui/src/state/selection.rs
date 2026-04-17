//! Current user selection across the three panes.
//!
//! `group` is the currently-highlighted group; `memory` is the slug
//! of the highlighted memory inside that group. Either can be
//! `None` — "nothing selected" is a legitimate state on first launch
//! or after a group is deleted remotely.

use mmcp_core::id::GroupId;

#[derive(Default, Debug, Clone)]
pub struct Selection {
    pub group: Option<GroupId>,
    pub memory: Option<String>,
}
