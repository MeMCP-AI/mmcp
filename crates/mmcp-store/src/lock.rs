//! Hierarchical scoped lock registry (FR-39 v2).
//!
//! Replaces the v1 per-group `Mutex` with `RwLock`-backed scopes
//! arranged in a flat parent-child hierarchy:
//!
//! ```text
//! Process
//! └── Group(g)
//!     └── Memory(uuid)
//! ```
//!
//! Concurrent agents that read different memories under the same
//! group should not block each other; coarse rename operations
//! that touch every memory under a slug must serialise the world
//! at the group level. Both shapes fall out of the standard
//! reader-writer-lock semantics applied per scope.
//!
//! ## Hierarchy contract
//!
//! Every operation acquires a chain of locks from the outermost
//! ancestor inward. The leaf-scope mode is the operation's
//! semantics; ancestor scopes are always taken `Shared` unless the
//! operation deliberately targets the ancestor (group-coarsening
//! renames, process-coarsening group creation).
//!
//! - **Memory-scoped read or write** (e.g. `mcp:read_memory`,
//!   `mcp:edit_memory`): `Shared Process` + `Shared Group(g)` +
//!   `(Shared|Exclusive) Memory(uuid)`.
//! - **Group-scoped create** (e.g. `code:add_feature`,
//!   `code:add_issue`, `code:import_memory` when writing a new
//!   slug): `Shared Process` + `Exclusive Group(g)`. The exclusive
//!   group lock gives the create flow a stable view of every
//!   existing memory regardless of kind, which the shared
//!   ticket-counter (`feature` + `issue` mint from one monotonic
//!   sequence) and the slug-uniqueness invariant both rely on.
//! - **Group-scoped coarsening write** (`mcp:rename_feature`,
//!   `mcp:rename_issue`): same chain as create — `Shared Process`
//!   + `Exclusive Group(g)`.
//! - **Process-scoped coarsening write** (`mcp:create_group`,
//!   `mcp:init_project`): `Exclusive Process`.
//!
//! The ancestor-prefix rule is what makes group-coarsening work:
//! every narrower write holds `Shared Group(g)` first, so an
//! `Exclusive Group(g)` request waits for them to drain and then
//! blocks every new narrower acquisition.
//!
//! ## Scope intentionally narrow
//!
//! - **Single process only.** A parallel `mmcp` CLI process
//!   targeting the same repo does NOT see this registry. A future
//!   slice can layer an `fs4` flock at
//!   `~/.mmcp/repos/<uuid>.git/mmcp.lock` for cross-process
//!   safety.
//! - **Non-reentrant.** Re-acquiring the same scope from the same
//!   task while a guard is held deadlocks; callers structure the
//!   public entrypoint to acquire once and have any internal
//!   helpers it calls operate without re-acquiring.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex as StdMutex};

use tokio::sync::{OwnedRwLockReadGuard, OwnedRwLockWriteGuard, RwLock};
use uuid::Uuid;

/// Lock scope. Each variant identifies a node in the hierarchy.
///
/// `Process` is the singleton root used by group-creation paths
/// that mutate the local mirror's directory layout.
/// `Group(uuid)` covers everything inside one group repo.
/// `Memory(uuid)` is the per-memory leaf.
///
/// The kind-partitioned `GroupKind` scope was retired when the
/// tracker chain (`feature` + `issue`) collapsed onto a single
/// per-group monotonic counter: serialising creates per-kind no
/// longer matches the invariant we need to protect, so the layer
/// went away.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum LockScope {
    /// Whole-process root. Held `Shared` by every per-group op,
    /// `Exclusive` only by group creation and similar mirror-wide
    /// mutations.
    Process,
    /// Whole-group scope. Held `Shared` by every per-memory op,
    /// `Exclusive` by group-coarsening writes (rename) and by
    /// kind-agnostic create paths that need a stable view of every
    /// existing memory in the group (ticket-counter mint).
    Group(Uuid),
    /// One memory's leaf scope. Held `Shared` by reads and
    /// `Exclusive` by writes; the ancestor chain ensures that a
    /// group-level coarsen blocks every in-flight Memory-scoped op
    /// via the prefix.
    Memory(Uuid),
}

/// Reader/writer mode for an `acquire` call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LockMode {
    /// Multiple holders may read concurrently.
    Shared,
    /// One holder; waits for readers to drain and blocks new ones.
    Exclusive,
}

/// Owning guard over one `acquire`d lock. Drop to release. Read
/// and write variants share a single drop point so callers can
/// hold a `Vec<ScopeGuard>` (the typical hierarchy stack) without
/// branching on the mode.
#[allow(dead_code, clippy::large_enum_variant)]
pub enum ScopeGuard {
    Read(OwnedRwLockReadGuard<()>),
    Write(OwnedRwLockWriteGuard<()>),
}

static REGISTRY: LazyLock<StdMutex<HashMap<LockScope, Arc<RwLock<()>>>>> =
    LazyLock::new(|| StdMutex::new(HashMap::new()));

fn lookup_or_install(scope: LockScope) -> Arc<RwLock<()>> {
    let mut registry = REGISTRY.lock().expect("lock-scope registry mutex poisoned");
    registry
        .entry(scope)
        .or_insert_with(|| Arc::new(RwLock::new(())))
        .clone()
}

/// Acquire a single scope at the requested mode. Async. Returns
/// the owned guard; drop to release.
///
/// Most callers want [`acquire_chain`] to take an entire
/// hierarchy stack in one call. `acquire` exists for the rare
/// site that already proved its ancestor coverage and only needs
/// one extra link.
pub async fn acquire(scope: LockScope, mode: LockMode) -> ScopeGuard {
    let lock = lookup_or_install(scope);
    match mode {
        LockMode::Shared => ScopeGuard::Read(lock.read_owned().await),
        LockMode::Exclusive => ScopeGuard::Write(lock.write_owned().await),
    }
}

/// Acquire every scope in `chain` in order. Returns the owning
/// guards in the same order so the caller can drop them in
/// reverse (deepest first) by dropping the `Vec`.
///
/// Always pass scopes from outermost ancestor to innermost leaf;
/// the registry does not enforce ordering, but a wrong order can
/// deadlock with another caller that took the inverse order.
pub async fn acquire_chain(chain: &[(LockScope, LockMode)]) -> Vec<ScopeGuard> {
    let mut guards = Vec::with_capacity(chain.len());
    for (scope, mode) in chain {
        guards.push(acquire(scope.clone(), *mode).await);
    }
    guards
}

/// Convenience: build the canonical chain for a memory-leaf
/// operation. `leaf_mode` is the leaf mode (`Shared` for reads,
/// `Exclusive` for writes); ancestors always ride `Shared`.
#[must_use]
pub fn memory_chain(group: Uuid, memory: Uuid, leaf_mode: LockMode) -> Vec<(LockScope, LockMode)> {
    vec![
        (LockScope::Process, LockMode::Shared),
        (LockScope::Group(group), LockMode::Shared),
        (LockScope::Memory(memory), leaf_mode),
    ]
}

/// Convenience: chain for a group-level create. The leaf is
/// `Exclusive Group(g)` because the create needs a stable view
/// of every existing memory under the group to mint the next
/// monotonic ticket number and to enforce slug uniqueness without
/// racing a sibling write.
#[must_use]
pub fn create_chain(group: Uuid) -> Vec<(LockScope, LockMode)> {
    vec![
        (LockScope::Process, LockMode::Shared),
        (LockScope::Group(group), LockMode::Exclusive),
    ]
}

/// Convenience: chain for a group-coarsening write (rename). One
/// entry — `Exclusive Group(g)` — under a `Shared Process` root
/// so it waits for every narrower in-flight op via the
/// ancestor-prefix and blocks every new one.
#[must_use]
pub fn coarsen_group_chain(group: Uuid) -> Vec<(LockScope, LockMode)> {
    vec![
        (LockScope::Process, LockMode::Shared),
        (LockScope::Group(group), LockMode::Exclusive),
    ]
}

/// Convenience: chain for a process-coarsening write
/// (`create_group`, `init_project`). Single `Exclusive Process`
/// entry; serialises across the whole mirror.
#[must_use]
pub fn coarsen_process_chain() -> Vec<(LockScope, LockMode)> {
    vec![(LockScope::Process, LockMode::Exclusive)]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    fn process_root() -> LockScope {
        LockScope::Process
    }

    fn group_scope(g: Uuid) -> LockScope {
        LockScope::Group(g)
    }

    /// Two writers contending on the same scope must not interleave.
    #[tokio::test]
    async fn exclusive_writers_serialise_per_scope() {
        let group = Uuid::now_v7();
        let active = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));

        let mut handles = Vec::new();
        for _ in 0..16 {
            let active = Arc::clone(&active);
            let peak = Arc::clone(&peak);
            handles.push(tokio::spawn(async move {
                let _g = acquire(group_scope(group), LockMode::Exclusive).await;
                let now = active.fetch_add(1, Ordering::SeqCst) + 1;
                peak.fetch_max(now, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(5)).await;
                active.fetch_sub(1, Ordering::SeqCst);
            }));
        }
        for h in handles {
            h.await.expect("task ok");
        }
        assert_eq!(peak.load(Ordering::SeqCst), 1);
    }

    /// Multiple readers on the same scope share concurrently.
    #[tokio::test]
    async fn shared_readers_run_concurrently() {
        let group = Uuid::now_v7();
        let active = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));

        let mut handles = Vec::new();
        for _ in 0..8 {
            let active = Arc::clone(&active);
            let peak = Arc::clone(&peak);
            handles.push(tokio::spawn(async move {
                let _g = acquire(group_scope(group), LockMode::Shared).await;
                let now = active.fetch_add(1, Ordering::SeqCst) + 1;
                peak.fetch_max(now, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(20)).await;
                active.fetch_sub(1, Ordering::SeqCst);
            }));
        }
        for h in handles {
            h.await.expect("task ok");
        }
        assert!(
            peak.load(Ordering::SeqCst) >= 2,
            "shared mode must let at least two readers run concurrently"
        );
    }

    /// Different groups don't contend on each other's scopes.
    #[tokio::test]
    async fn different_groups_do_not_contend() {
        let a = Uuid::now_v7();
        let b = Uuid::now_v7();
        let _held_a = acquire(group_scope(a), LockMode::Exclusive).await;
        // Acquiring B under a short timeout must succeed.
        let b_guard = tokio::time::timeout(
            Duration::from_millis(200),
            acquire(group_scope(b), LockMode::Exclusive),
        )
        .await
        .expect("group B must not be gated by group A's lock");
        drop(b_guard);
    }

    /// Ancestor-prefix rule: a thread holding `Shared Group(g)`
    /// blocks every other thread that asks for `Exclusive Group(g)`,
    /// no matter what their leaf intent was. The narrower writer
    /// can therefore safely rely on the ancestor lock as a barrier
    /// against coarsening writes.
    #[tokio::test]
    async fn exclusive_group_blocks_until_shared_holders_release() {
        let group = Uuid::now_v7();
        let shared = acquire(group_scope(group), LockMode::Shared).await;
        // Should NOT be able to acquire Exclusive while the shared
        // holder is alive.
        let coarsen_attempt = tokio::time::timeout(
            Duration::from_millis(100),
            acquire(group_scope(group), LockMode::Exclusive),
        )
        .await;
        assert!(
            coarsen_attempt.is_err(),
            "Exclusive Group must wait for Shared holders to drain"
        );
        drop(shared);
        // After drop the coarsen succeeds quickly.
        let _exclusive = tokio::time::timeout(
            Duration::from_millis(100),
            acquire(group_scope(group), LockMode::Exclusive),
        )
        .await
        .expect("after the shared holder drops, Exclusive must succeed")
        ;
    }

    /// Memory-leaf chain: the canonical helper produces ancestors
    /// all `Shared` and the leaf at the requested mode.
    #[test]
    fn memory_chain_sets_ancestors_shared_and_leaf_mode() {
        let g = Uuid::now_v7();
        let m = Uuid::now_v7();
        let chain = memory_chain(g, m, LockMode::Exclusive);
        assert_eq!(chain.len(), 3);
        assert_eq!(chain[0], (LockScope::Process, LockMode::Shared));
        assert_eq!(chain[1], (LockScope::Group(g), LockMode::Shared));
        assert_eq!(chain[2], (LockScope::Memory(m), LockMode::Exclusive));
    }

    /// Group-creation helper produces the two-entry chain that
    /// the tracker create flows rely on.
    #[test]
    fn create_chain_is_process_shared_plus_group_exclusive() {
        let g = Uuid::now_v7();
        let chain = create_chain(g);
        assert_eq!(
            chain,
            vec![
                (process_root(), LockMode::Shared),
                (LockScope::Group(g), LockMode::Exclusive),
            ]
        );
    }

    /// Group-coarsening helper produces the two-entry chain that
    /// the rename operation relies on.
    #[test]
    fn coarsen_group_chain_is_process_shared_plus_group_exclusive() {
        let g = Uuid::now_v7();
        let chain = coarsen_group_chain(g);
        assert_eq!(
            chain,
            vec![
                (process_root(), LockMode::Shared),
                (LockScope::Group(g), LockMode::Exclusive),
            ]
        );
    }

    /// Process coarsening chain is the singleton Exclusive Process.
    #[test]
    fn coarsen_process_chain_is_exclusive_process_only() {
        let chain = coarsen_process_chain();
        assert_eq!(chain, vec![(process_root(), LockMode::Exclusive)]);
    }

    /// End-to-end: a coarsening rename (Exclusive Group) blocks a
    /// narrower memory-modify chain that holds Shared Group via the
    /// ancestor-prefix rule.
    #[tokio::test]
    async fn coarsen_group_blocks_narrower_memory_chain_via_ancestor_prefix() {
        let g = Uuid::now_v7();
        let m = Uuid::now_v7();
        // Acquire a memory-modify chain (Shared on Group, Exclusive
        // on Memory).
        let _memory_guards =
            acquire_chain(&memory_chain(g, m, LockMode::Exclusive)).await;
        // A coarsening rename now wants Exclusive Group; it must wait.
        let coarsen_attempt = tokio::time::timeout(
            Duration::from_millis(100),
            acquire_chain(&coarsen_group_chain(g)),
        )
        .await;
        assert!(
            coarsen_attempt.is_err(),
            "coarsening Exclusive Group must wait for narrower Shared Group holders"
        );
    }
}
