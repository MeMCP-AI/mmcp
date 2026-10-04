//! Owner of a running periodic task.

use tokio::task::JoinHandle;

/// Owns a task spawned by [`crate::background::spawn_periodic_task`].
///
/// Dropping the handle aborts the task, so a task never outlives the server that started it.
#[derive(Debug)]
pub struct PeriodicTaskHandle {
    task: JoinHandle<()>,
}

impl PeriodicTaskHandle {
    pub(super) fn new(task: JoinHandle<()>) -> Self {
        Self { task }
    }

    /// Whether the task has ended, which only an abort or a panic of a tick causes.
    #[must_use]
    pub fn is_finished(&self) -> bool {
        self.task.is_finished()
    }
}

impl Drop for PeriodicTaskHandle {
    fn drop(&mut self) {
        self.task.abort();
    }
}
