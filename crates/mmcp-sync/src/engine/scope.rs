//! Group-scope matching used to decide whether a group is in-scope
//! for a given [`crate::SyncFilter`].

use mmcp_core::manifest::GroupScope;
use uuid::Uuid;

use crate::filter::{ScopeIndex, SyncFilter};

/// True when `group_id` satisfies `filter`.
///
/// `SyncFilter::All` always matches; `SyncFilter::Group(u)` is a
/// straight UUID compare; `SyncFilter::Scope(s)` consults
/// `scope_index` and treats unknown groups as non-matching so the
/// engine silently skips them (unknown-group operator errors are
/// raised at the CLI / MCP boundary, not here).
pub(super) fn group_matches(
    filter: SyncFilter,
    group_id: Uuid,
    scope_index: &dyn ScopeIndex,
) -> bool {
    match filter {
        SyncFilter::All => true,
        SyncFilter::Group(target) => target == group_id,
        SyncFilter::Scope(target) => scope_index.scope_of(group_id) == Some(target),
    }
}

/// Minimal no-op scope index for callers that only ever pass
/// [`SyncFilter::All`]. The engine's filter dispatch short-circuits
/// on `All` before consulting the scope index, so `scope_of` here
/// is never actually called in that mode; the type exists so test
/// fixtures and CLI paths that do not have a real scope index can
/// pass a trivial placeholder rather than constructing one.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoScopeIndex;

impl ScopeIndex for NoScopeIndex {
    fn scope_of(&self, _group_id: Uuid) -> Option<GroupScope> {
        None
    }
}
