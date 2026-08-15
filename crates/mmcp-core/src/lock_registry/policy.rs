//! Prune-trigger policy for [`super::registry::KeyedLockRegistry`].

/// When a [`super::registry::KeyedLockRegistry`] sweeps its idle entries.
///
/// An idle entry is one whose `Arc::strong_count() == 1`: nobody but
/// the registry's own map slot holds a reference.
/// Two established call sites need two different triggers.
/// One prunes on every lookup because its keyspace is naturally
/// low-churn and low-cardinality.
/// The other prunes only once the registry has grown past a
/// threshold, because its keyspace is higher-churn and an O(n) sweep
/// on every lookup would cost more than the memory it reclaims below
/// that size.
/// Kept as an open enum, never a bool, so a future trigger (for
/// example time-based) is an additive variant, never a breaking
/// change to callers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrunePolicy {
    /// Sweep idle entries on every [`super::registry::KeyedLockRegistry::get_or_install`] call.
    Always,
    /// Sweep idle entries only once the registry holds at least this many entries.
    Threshold(usize),
}
