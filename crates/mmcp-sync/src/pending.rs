//! Pending push queue.
//!
//! Models the local queue of edits that have been committed to the
//! client's local git clone but not yet pushed to the server. The
//! queue is an in-memory `Mutex<Vec<_>>`; persistent storage
//! (carrying edits across process restarts) will live in a future
//! `mmcp-db` table once we decide whether client-side pending state
//! belongs in the same SQLite file as the session mirror.

use std::sync::{Arc, Mutex};

use jiff::Timestamp;
use mmcp_core::memory::BumpIntent;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::SyncError;

/// One pending edit waiting to be pushed to the server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingEdit {
    /// Stable identifier for the queue entry.
    pub id: Uuid,

    /// Group the edit belongs to. Needed so the sync engine can
    /// filter the drain queue by `GroupScope` or by a caller-supplied
    /// group UUID without a round-trip to the group index. Without
    /// this, per-group push scoping would need an extra lookup per
    /// edit; with it, drain is a linear filter.
    pub group_id: Uuid,

    /// Memory the edit applies to.
    pub memory: Uuid,

    /// Commit hash in the local clone.
    pub commit: String,

    /// Bump intent requested by the editor.
    pub bump: BumpIntent,

    /// Commit message supplied alongside the edit.
    pub message: String,

    /// Milliseconds since epoch when the edit was enqueued.
    pub enqueued_at: i64,
}

impl PendingEdit {
    #[must_use]
    pub fn new(
        group_id: Uuid,
        memory: Uuid,
        commit: impl Into<String>,
        bump: BumpIntent,
        message: impl Into<String>,
    ) -> Self {
        Self {
            id: Uuid::now_v7(),
            group_id,
            memory,
            commit: commit.into(),
            bump,
            message: message.into(),
            enqueued_at: Timestamp::now().as_millisecond(),
        }
    }
}

/// Thread-safe in-memory pending push queue.
#[derive(Debug, Clone, Default)]
pub struct PendingQueue {
    inner: Arc<Mutex<Vec<PendingEdit>>>,
}

impl PendingQueue {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Append an edit to the tail of the queue.
    pub fn enqueue(&self, edit: PendingEdit) {
        let mut guard = self.inner.lock().expect("poisoned lock");
        guard.push(edit);
    }

    /// Number of edits waiting in the queue.
    #[must_use]
    pub fn len(&self) -> usize {
        self.inner.lock().expect("poisoned lock").len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.inner.lock().expect("poisoned lock").is_empty()
    }

    /// Remove the oldest edit from the queue and return it.
    #[must_use]
    pub fn dequeue(&self) -> Option<PendingEdit> {
        let mut guard = self.inner.lock().expect("poisoned lock");
        if guard.is_empty() {
            None
        } else {
            Some(guard.remove(0))
        }
    }

    /// Snapshot of the current queue contents, cheap for callers
    /// that want to display the state without holding the lock.
    #[must_use]
    pub fn snapshot(&self) -> Vec<PendingEdit> {
        self.inner.lock().expect("poisoned lock").clone()
    }

    /// Remove an edit by its id. Returns the removed entry, or
    /// [`SyncError::NotFound`] if the id is not present.
    pub fn remove(&self, id: Uuid) -> Result<PendingEdit, SyncError> {
        let mut guard = self.inner.lock().expect("poisoned lock");
        let pos = guard
            .iter()
            .position(|e| e.id == id)
            .ok_or_else(|| SyncError::NotFound(id.to_string()))?;
        Ok(guard.remove(pos))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enqueue_dequeue_is_fifo() {
        let q = PendingQueue::new();
        let a = PendingEdit::new(Uuid::now_v7(), Uuid::now_v7(), "aaa", BumpIntent::Patch, "a");
        let b = PendingEdit::new(Uuid::now_v7(), Uuid::now_v7(), "bbb", BumpIntent::Minor, "b");
        q.enqueue(a.clone());
        q.enqueue(b.clone());
        assert_eq!(q.len(), 2);
        assert_eq!(q.dequeue(), Some(a));
        assert_eq!(q.dequeue(), Some(b));
        assert!(q.is_empty());
    }

    #[test]
    fn is_empty_flips_with_queue_state() {
        // Pin both polarities of `is_empty` so a mutation replacing
        // the body with `true` (which cargo-mutants flagged as a
        // surviving mutant in an earlier pass) no longer escapes -
        // the non-empty branch immediately produces a disagreement.
        let q = PendingQueue::new();
        assert!(q.is_empty(), "fresh queue must be empty");
        q.enqueue(PendingEdit::new(
            Uuid::now_v7(),
            Uuid::now_v7(),
            "x",
            BumpIntent::Patch,
            "x",
        ));
        assert!(!q.is_empty(), "queue with one edit is not empty");
    }

    #[test]
    fn snapshot_is_independent_of_queue() {
        let q = PendingQueue::new();
        q.enqueue(PendingEdit::new(
            Uuid::now_v7(),
            Uuid::now_v7(),
            "a",
            BumpIntent::Patch,
            "a",
        ));
        let snap = q.snapshot();
        assert_eq!(snap.len(), 1);
        q.enqueue(PendingEdit::new(
            Uuid::now_v7(),
            Uuid::now_v7(),
            "b",
            BumpIntent::Patch,
            "b",
        ));
        assert_eq!(snap.len(), 1);
        assert_eq!(q.len(), 2);
    }

    #[test]
    fn remove_by_id_works() {
        let q = PendingQueue::new();
        let a = PendingEdit::new(Uuid::now_v7(), Uuid::now_v7(), "aaa", BumpIntent::Patch, "a");
        let b = PendingEdit::new(Uuid::now_v7(), Uuid::now_v7(), "bbb", BumpIntent::Minor, "b");
        let c = PendingEdit::new(Uuid::now_v7(), Uuid::now_v7(), "ccc", BumpIntent::Major, "c");
        q.enqueue(a.clone());
        q.enqueue(b.clone());
        q.enqueue(c.clone());
        let removed = q.remove(b.id).unwrap();
        assert_eq!(removed, b);
        assert_eq!(q.snapshot(), vec![a, c]);
    }

    #[test]
    fn remove_unknown_id_returns_not_found() {
        let q = PendingQueue::new();
        let err = q.remove(Uuid::now_v7()).unwrap_err();
        assert!(matches!(err, SyncError::NotFound(_)));
    }
}
