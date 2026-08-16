//! Generic keyed, reference-counted, self-pruning lock/handle registry.
//!
//! The shared owner for `mmcp-server`'s per-group git write-lock map
//! and `mmcp-store`'s hierarchical scope-lock map.
//! Both lazily insert-or-fetch an `Arc<P>` per key and prune entries
//! nobody but the registry itself still holds
//! (`Arc::strong_count(handle) == 1`).
//! The two consumers differ only in WHEN they prune, every call
//! versus threshold-gated; both triggers are first-class via
//! [`PrunePolicy`], picked by the consumer at construction.

pub mod policy;
pub mod registry;

pub use policy::PrunePolicy;
pub use registry::KeyedLockRegistry;
