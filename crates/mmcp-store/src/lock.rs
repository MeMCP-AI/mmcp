//! Per-group write-serialisation lock.
//!
//! FR-39 v1: the observed failure mode is two concurrent
//! `add_feature` calls from different agent sessions racing on
//! `next_feature_number` and both assigning `max + 1` to the same
//! value. This module ships a process-wide async mutex registry
//! keyed by group UUID so every write path that mutates state
//! serialises on the same per-group gate.
//!
//! Scope intentionally narrow:
//!
//! - **Single process only.** The registry lives in this
//!   process's static; a parallel `mmcp` CLI invocation targeting
//!   the same group repo does NOT see this lock. A follow-up can
//!   layer an `fs4` flock on `~/.mmcp/repos/<uuid>.git/mmcp.lock`
//!   for cross-process safety; the current design leaves the
//!   filesystem path unused.
//! - **Coarse.** The whole logical operation (read + write)
//!   holds the lock, not just the commit step. That preserves
//!   read-then-write invariants like auto-numbering.
//! - **Non-reentrant.** Nested re-locks from the same task
//!   deadlock; callers structure their code so the public
//!   entrypoint acquires once and any internal write primitives
//!   it calls are unlocked siblings.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex as StdMutex};

use tokio::sync::{Mutex, OwnedMutexGuard};
use uuid::Uuid;

static REGISTRY: LazyLock<StdMutex<HashMap<Uuid, Arc<Mutex<()>>>>> =
    LazyLock::new(|| StdMutex::new(HashMap::new()));

/// Acquire the per-group write lock, blocking (async) until no
/// other task holds it for the same `group`.
///
/// Drop the returned guard to release; the common pattern is
/// `let _guard = acquire_group_lock(uuid).await;` at the top of a
/// write-side function body.
///
/// Panics only if the std Mutex guarding the registry is
/// poisoned; since this process never panics while holding it,
/// that can only happen if a destructor in a dependent task
/// panicked, which we cannot safely recover from anyway.
pub async fn acquire_group_lock(group: Uuid) -> OwnedMutexGuard<()> {
    let mutex = {
        let mut registry = REGISTRY
            .lock()
            .expect("group-lock registry mutex poisoned");
        registry
            .entry(group)
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    };
    mutex.lock_owned().await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    #[tokio::test]
    async fn concurrent_acquirers_serialise_per_group() {
        // Two tasks contending on the same group must not
        // observe interleaved critical sections. The counter
        // reaches `1` inside the section and `0` outside; if
        // the lock ever lets both in the max observed value
        // would be `2`.
        let group = Uuid::now_v7();
        let active = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));

        let mut handles = Vec::new();
        for _ in 0..16 {
            let active = Arc::clone(&active);
            let peak = Arc::clone(&peak);
            handles.push(tokio::spawn(async move {
                let _guard = acquire_group_lock(group).await;
                let now = active.fetch_add(1, Ordering::SeqCst) + 1;
                peak.fetch_max(now, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(5)).await;
                active.fetch_sub(1, Ordering::SeqCst);
            }));
        }

        for h in handles {
            h.await.expect("task ok");
        }
        assert_eq!(
            peak.load(Ordering::SeqCst),
            1,
            "per-group lock must keep concurrent writers serial"
        );
    }

    #[tokio::test]
    async fn different_groups_do_not_contend() {
        // Lock on group A does not block acquisition on group B;
        // the registry keys are independent.
        let a = Uuid::now_v7();
        let b = Uuid::now_v7();
        let held_a = acquire_group_lock(a).await;
        // Acquiring B under a short timeout must succeed.
        let b_guard =
            tokio::time::timeout(Duration::from_millis(200), acquire_group_lock(b))
                .await
                .expect("group B must not be gated by group A's lock");
        drop(b_guard);
        drop(held_a);
    }
}
