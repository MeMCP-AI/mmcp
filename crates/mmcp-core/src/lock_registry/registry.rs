//! Generic keyed, reference-counted, self-pruning lock/handle registry.

use std::collections::HashMap;
use std::hash::Hash;
use std::sync::{Arc, Mutex as StdMutex};

use crate::lock_registry::policy::PrunePolicy;

/// Keyed registry over `Arc<P>` handles, lazily installed on first lookup.
///
/// Generic over the key type `K` and the guarded primitive `P` (for
/// example `tokio::sync::Mutex<()>` or `tokio::sync::RwLock<()>`).
/// `mmcp-core` stays free of an async-runtime dependency because it
/// never constructs or awaits `P` itself, only stores and hands out
/// `Arc<P>` clones built by the caller-supplied `factory`.
pub struct KeyedLockRegistry<K, P> {
    entries: StdMutex<HashMap<K, Arc<P>>>,
    policy: PrunePolicy,
    factory: fn() -> P,
}

impl<K, P> KeyedLockRegistry<K, P>
where
    K: Eq + Hash,
{
    /// Build an empty registry that prunes idle entries per `policy`.
    ///
    /// `factory` constructs a fresh `P` on first lookup of a key.
    #[must_use]
    pub fn new(policy: PrunePolicy, factory: fn() -> P) -> Self {
        Self {
            entries: StdMutex::new(HashMap::new()),
            policy,
            factory,
        }
    }

    /// Look up (or lazily install) the `Arc<P>` backing `key`, pruning idle entries first per `self.policy`.
    ///
    /// Race-free pruning: every caller that intends to use a key's
    /// handle has already cloned its `Arc` (bumping the strong count)
    /// via this same function, under this same mutex, before
    /// releasing it.
    /// A concurrent acquirer therefore either already holds its clone
    /// (`strong_count > 1`, survives the sweep) or has not yet
    /// reached this function at all (nothing to race).
    /// Because the sweep runs before the lookup, `key` itself can be
    /// pruned and reinstalled fresh within this same call when it was
    /// idle going in; the returned handle is correct either way.
    #[must_use]
    pub fn get_or_install(&self, key: K) -> Arc<P> {
        // SAFETY: poison only happens if another thread panicked while
        // holding this mutex, leaving `entries` in a possibly
        // inconsistent state. That is an unrecoverable invariant
        // break, not a recoverable condition to route through
        // `Result`, so propagating the panic here is correct per the
        // project's least-blocking-runtime rule.
        #[allow(clippy::expect_used)]
        let mut entries = self.entries.lock().expect(
            "keyed lock registry mutex poisoned: another thread panicked while holding it, \
             leaving the map in a possibly inconsistent state that must not be silently \
             continued past",
        );
        let should_prune = match self.policy {
            PrunePolicy::Always => true,
            PrunePolicy::Threshold(threshold) => entries.len() >= threshold,
        };
        if should_prune {
            entries.retain(|_, handle| Arc::strong_count(handle) > 1);
        }
        entries
            .entry(key)
            .or_insert_with(|| Arc::new((self.factory)()))
            .clone()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::thread;
    use std::time::Duration;

    fn std_mutex_factory() -> StdMutex<()> {
        StdMutex::new(())
    }

    #[test]
    fn get_or_install_returns_same_handle_for_same_key() {
        let registry = KeyedLockRegistry::new(PrunePolicy::Always, std_mutex_factory);
        let a = registry.get_or_install(7u32);
        let b = registry.get_or_install(7u32);
        assert!(Arc::ptr_eq(&a, &b), "same key must hand out the same Arc");
    }

    #[test]
    fn get_or_install_returns_distinct_handles_for_distinct_keys() {
        let registry = KeyedLockRegistry::new(PrunePolicy::Always, std_mutex_factory);
        let a = registry.get_or_install(1u32);
        let b = registry.get_or_install(2u32);
        assert!(!Arc::ptr_eq(&a, &b), "distinct keys must not share an Arc");
    }

    /// `Always` sweeps on every call, so an idle entry (no outstanding
    /// clone) is pruned and reinstalled fresh on the very next lookup,
    /// proven via `Weak::upgrade` rather than a freed pointer address
    /// (the allocator can reuse an identical-layout freed block, which
    /// would make a raw-pointer comparison a false negative).
    #[test]
    fn always_policy_prunes_idle_entry_on_next_lookup() {
        let registry = KeyedLockRegistry::new(PrunePolicy::Always, std_mutex_factory);
        let a = registry.get_or_install(1u32);
        let weak = Arc::downgrade(&a);
        drop(a);

        let _b = registry.get_or_install(2u32);
        assert!(
            weak.upgrade().is_none(),
            "Always must prune the idle entry on the very next lookup"
        );
    }

    /// `Threshold` never sweeps below the threshold, so an idle entry
    /// survives; once the map reaches the threshold, the next lookup's
    /// pre-insert check sweeps every idle entry, including the one
    /// just looked up.
    #[test]
    fn threshold_policy_prunes_only_once_len_reaches_threshold() {
        let registry = KeyedLockRegistry::new(PrunePolicy::Threshold(3), std_mutex_factory);

        let a = registry.get_or_install(1u32);
        let weak = Arc::downgrade(&a);
        drop(a);

        // len=1 after key 1; below threshold, no sweep.
        let b = registry.get_or_install(2u32);
        drop(b);
        assert!(
            weak.upgrade().is_some(),
            "idle entry must survive below the prune threshold"
        );

        // len=2 after key 2, still below threshold(3); the survival
        // check above already proved it. Grow to len=3 (all idle),
        // then a fourth lookup's pre-insert check sees len >= 3 and
        // sweeps both idle entries before installing key 3.
        let c = registry.get_or_install(3u32);
        drop(c);
        let _d = registry.get_or_install(4u32);
        assert!(
            weak.upgrade().is_none(),
            "once the registry reaches the threshold, idle entries must be pruned"
        );
    }

    /// An entry with an outstanding `Arc` clone (`strong_count > 1`,
    /// i.e. actively held) must never be pruned, under either policy.
    #[test]
    fn prune_never_evicts_a_still_held_entry() {
        let registry = KeyedLockRegistry::new(PrunePolicy::Always, std_mutex_factory);
        let held = registry.get_or_install(1u32);

        for other_key in 2u32..50 {
            let _ = registry.get_or_install(other_key);
        }

        let still = registry.get_or_install(1u32);
        assert!(
            Arc::ptr_eq(&held, &still),
            "an entry with an outstanding clone must never be pruned"
        );
    }

    /// Same-key contention must actually serialize through the
    /// handed-out primitive, not merely share an `Arc` in name.
    #[test]
    fn same_key_contention_actually_serializes() {
        let registry = Arc::new(KeyedLockRegistry::new(
            PrunePolicy::Always,
            std_mutex_factory,
        ));
        let active = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));

        const CONTENDING_THREADS: usize = 8;
        let handles: Vec<_> = (0..CONTENDING_THREADS)
            .map(|_| {
                let registry = Arc::clone(&registry);
                let active = Arc::clone(&active);
                let peak = Arc::clone(&peak);
                thread::spawn(move || {
                    let lock = registry.get_or_install(42u32);
                    let _guard = lock.lock().expect("mutex poisoned");
                    let now = active.fetch_add(1, Ordering::SeqCst) + 1;
                    peak.fetch_max(now, Ordering::SeqCst);
                    thread::sleep(Duration::from_millis(5));
                    active.fetch_sub(1, Ordering::SeqCst);
                })
            })
            .collect();
        for handle in handles {
            handle.join().expect("thread ok");
        }
        assert_eq!(
            peak.load(Ordering::SeqCst),
            1,
            "contending threads on the same key must serialize"
        );
    }
}
