//! Lifecycle shared by every periodic background task.

use std::fmt::Debug;
use std::future::Future;
use std::time::Duration;

use super::PeriodicTaskHandle;

/// Spawn a task that runs `tick` every `period`, forever.
///
/// The first tick runs at once.
/// A failed tick is logged under `task_name` and retried at the next period, so it never stops the task.
/// Dropping the returned handle aborts the task.
/// Abort lands between ticks or inside one, so each tick must be safe to stop at any await point.
pub fn spawn_periodic_task<Tick, TickFuture, TickError>(
    task_name: &'static str,
    period: Duration,
    mut tick: Tick,
) -> PeriodicTaskHandle
where
    Tick: FnMut() -> TickFuture + Send + 'static,
    TickFuture: Future<Output = Result<(), TickError>> + Send,
    TickError: Debug,
{
    PeriodicTaskHandle::new(tokio::spawn(async move {
        let mut interval = tokio::time::interval(period);
        loop {
            interval.tick().await;
            if let Err(error) = tick().await {
                tracing::error!(task = task_name, error = ?error, "periodic task tick failed");
            }
        }
    }))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    const TEST_PERIOD: Duration = Duration::from_millis(5);
    /// Upper bound on a wait, never a pause the assertions depend on.
    const WAIT_LIMIT: Duration = Duration::from_secs(10);
    /// Ticks the tests wait for, enough to prove the task keeps running.
    const TICKS_TO_OBSERVE: usize = 3;
    /// A period no test waits out, so only the immediate first tick can run.
    const UNREACHED_PERIOD: Duration = Duration::from_secs(3600);
    /// Time for an abort that landed mid-tick to settle.
    const ABORT_SETTLE_TIME: Duration = Duration::from_millis(20);
    /// Time during which a task that kept running would tick several times.
    const QUIET_OBSERVATION_TIME: Duration = Duration::from_millis(50);

    async fn wait_for_ticks(counter: &AtomicUsize, expected: usize) {
        tokio::time::timeout(WAIT_LIMIT, async {
            while counter.load(Ordering::SeqCst) < expected {
                tokio::time::sleep(TEST_PERIOD).await;
            }
        })
        .await
        .expect("the task must keep ticking");
    }

    #[tokio::test]
    async fn periodic_task_ticks_repeatedly_starting_at_once() {
        let ticks = Arc::new(AtomicUsize::new(0));
        let counter = ticks.clone();
        // A period no test waits out proves the first tick does not wait for the period.
        let handle = spawn_periodic_task("test", UNREACHED_PERIOD, move || {
            let counter = counter.clone();
            async move {
                counter.fetch_add(1, Ordering::SeqCst);
                Ok::<(), std::convert::Infallible>(())
            }
        });

        wait_for_ticks(&ticks, 1).await;
        drop(handle);
    }

    #[tokio::test]
    async fn periodic_task_keeps_running_after_a_failed_tick() {
        let ticks = Arc::new(AtomicUsize::new(0));
        let counter = ticks.clone();
        let handle = spawn_periodic_task("test", TEST_PERIOD, move || {
            let counter = counter.clone();
            async move {
                counter.fetch_add(1, Ordering::SeqCst);
                Err::<(), &'static str>("tick failed")
            }
        });

        wait_for_ticks(&ticks, TICKS_TO_OBSERVE).await;
        assert!(!handle.is_finished(), "a failed tick must not end the task");
    }

    #[tokio::test]
    async fn dropping_the_handle_stops_the_ticks() {
        let ticks = Arc::new(AtomicUsize::new(0));
        let counter = ticks.clone();
        let handle = spawn_periodic_task("test", TEST_PERIOD, move || {
            let counter = counter.clone();
            async move {
                counter.fetch_add(1, Ordering::SeqCst);
                Ok::<(), std::convert::Infallible>(())
            }
        });
        wait_for_ticks(&ticks, 1).await;

        drop(handle);
        // Let an abort that landed mid-tick settle before taking the reference count.
        tokio::time::sleep(ABORT_SETTLE_TIME).await;
        let after_drop = ticks.load(Ordering::SeqCst);
        tokio::time::sleep(QUIET_OBSERVATION_TIME).await;

        assert_eq!(
            ticks.load(Ordering::SeqCst),
            after_drop,
            "no tick may run after the handle is dropped"
        );
    }
}
