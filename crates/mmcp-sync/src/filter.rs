//! Sync filter taxonomy and the scope-lookup trait the engine
//! needs to apply it.
//!
//! Every effectful sync call (`pull` / `push` / `sync`) takes a [`SyncFilter`] that specifies which groups it targets.
//! The three variants map one-to-one onto the operator-facing selectors:
//!
//! - [`SyncFilter::Group`]: a single group, addressed by UUID.
//!   The caller resolved slug-to-UUID already; the engine never touches text.
//! - [`SyncFilter::Scope`]: every locally-known group whose [`GroupScope`] matches.
//!   The engine asks the caller-supplied [`ScopeIndex`] for each group's scope at drain / fetch time.
//! - [`SyncFilter::All`]: explicit fanout across the whole mirror, with no scope restriction.
//!
//! Zero selectors and multiple-selector combinations are rejected at the CLI / MCP boundary:
//! clap `ArgGroup` on the CLI side, `resolve_sync_filter` on the MCP side.
//! The engine never sees an ambiguous filter.
//!
//! The [`ScopeIndex`] trait is narrow on purpose: the engine only needs to answer "what scope is group X?".
//! Handle resolution is still the [`crate::engine::GroupHandleResolver`] trait's job;
//! the two concerns stay decoupled so callers that already have an in-memory scope map (test fixtures)
//! don't need to wire a full group index to drive a scoped drain.

use mmcp_core::manifest::GroupScope;
use uuid::Uuid;

/// Caller-supplied selector for sync operations.
///
/// Required argument on `SyncEngine::pull` / `push` / `sync`.
/// The variants are intentionally the only way a caller can scope the operation -
/// there is no implicit "all" default on the engine side,
/// so a newly-added sync entrypoint cannot accidentally inherit a whole-mirror default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncFilter {
    /// Target exactly one group by UUID.
    Group(Uuid),
    /// Target every locally-known group whose manifest carries the
    /// given [`GroupScope`].
    Scope(GroupScope),
    /// Explicit opt-in to the whole local mirror.
    /// Must be supplied literally by the caller; the engine never synthesizes it.
    All,
}

/// Look up a group's [`GroupScope`] by UUID.
///
/// Implemented by `mmcp-store`'s `GroupIndex` in the next commit;
/// the engine depends on the trait so test fixtures can plug a small in-memory map in without dragging the store layer.
/// Narrow-by-design: the only reason the engine talks to the scope index is to filter by `GroupScope`,
/// so the trait has one method.
///
/// `Send + Sync` supertraits mirror [`crate::GroupHandleResolver`]
/// so the engine's async methods can spawn onto a multi-threaded runtime.
/// The MCP tool router boxes returned futures with a `Send` bound.
pub trait ScopeIndex: Send + Sync {
    /// Return the scope recorded for `group_id` if the index knows about it,
    /// or `None` when the group is absent.
    /// The engine treats absence as "does not match any scope" -
    /// a [`SyncFilter::Scope`] drain silently skips unknown groups,
    /// and the CLI / MCP layer is where operator-visible errors about unknown groups are raised.
    fn scope_of(&self, group_id: Uuid) -> Option<GroupScope>;
}

/// Blanket impl so `&T` forwards to `T` - lets callers hand a
/// borrowed index to the engine without ceremony.
impl<T: ScopeIndex + ?Sized> ScopeIndex for &T {
    fn scope_of(&self, group_id: Uuid) -> Option<GroupScope> {
        (**self).scope_of(group_id)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use std::collections::HashMap;

    /// In-memory scope index used by downstream engine tests. Lives
    /// alongside the trait so the trait's contract stays testable
    /// from inside this crate without pulling in `mmcp-store`.
    #[derive(Debug, Default)]
    pub struct MemoryScopeIndex {
        entries: HashMap<Uuid, GroupScope>,
    }

    impl MemoryScopeIndex {
        pub fn insert(&mut self, group_id: Uuid, scope: GroupScope) {
            self.entries.insert(group_id, scope);
        }
    }

    impl ScopeIndex for MemoryScopeIndex {
        fn scope_of(&self, group_id: Uuid) -> Option<GroupScope> {
            self.entries.get(&group_id).copied()
        }
    }

    #[test]
    fn sync_filter_variants_are_copy() {
        // Copy is load-bearing - the engine's filter dispatch
        // destructures the enum inline rather than cloning. Pin it
        // so a future field addition doesn't silently regress.
        fn assert_copy<T: Copy>() {}
        assert_copy::<SyncFilter>();
    }

    #[test]
    fn scope_index_lookup_returns_recorded_scope() {
        let group = Uuid::now_v7();
        let mut index = MemoryScopeIndex::default();
        index.insert(group, GroupScope::Shared);
        assert_eq!(index.scope_of(group), Some(GroupScope::Shared));
    }

    #[test]
    fn scope_index_returns_none_for_unknown_group() {
        let index = MemoryScopeIndex::default();
        assert!(index.scope_of(Uuid::now_v7()).is_none());
    }

    #[test]
    fn scope_index_blanket_impl_forwards_through_reference() {
        let group = Uuid::now_v7();
        let mut index = MemoryScopeIndex::default();
        index.insert(group, GroupScope::Global);
        let as_ref: &dyn ScopeIndex = &index;
        assert_eq!(as_ref.scope_of(group), Some(GroupScope::Global));
    }
}
