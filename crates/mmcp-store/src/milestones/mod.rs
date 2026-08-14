//! Milestone domain: typed CRUD over milestone memories,
//! plus the live rollup computed over their linked features.
//!
//! Split into two concern-named submodules under this shared parent folder.
//!
//! - [`crud`]: add/read/update/list over milestone memories.
//!   Sister surface to [`crate::features`]/[`crate::issues`], but a deliberately reduced surface:
//!   add, read, update, list, no delete, no rename, no supersede flow,
//!   no `depends_on`/`blocks` cross-refs, no shared ticket-number counter.
//! - [`rollup`]: the live, computed [`rollup::RollupStatus`] fold over a milestone's own-group features.
//!   A milestone's own frontmatter carries only an editorial `MilestoneStatus`;
//!   the live status is never persisted, and every read path in [`crud`] computes it fresh via [`rollup::compute`].

pub mod crud;
pub mod rollup;

pub use crud::{
    AddSpec, MilestoneError, MilestoneRecord, UpdateSpec, add_milestone, list_milestones,
    read_milestone, update_milestone,
};
pub use rollup::{MilestoneRollup, RollupStatus};
