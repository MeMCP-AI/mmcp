//! Background sweep deleting expired session records.

use std::time::Duration;

use tokio::task::JoinHandle;

use crate::defaults::{EXPIRED_SESSION_SWEEP_BATCH_SIZE, EXPIRED_SESSION_SWEEP_INTERVAL};
use crate::session_store::DatabaseSessionStore;

/// Spawn the sweep with the production period and batch size.
///
/// The caller aborts the returned handle when the server stops.
/// Abort is safe at any point, because each batch is one atomic `DELETE`.
pub fn spawn_expired_session_sweeper(store: DatabaseSessionStore) -> JoinHandle<()> {
    tracing::info!(
        sweep_period_secs = EXPIRED_SESSION_SWEEP_INTERVAL.as_secs(),
        sweep_batch_size = EXPIRED_SESSION_SWEEP_BATCH_SIZE,
    );
    tokio::spawn(run_expired_session_sweeper(
        store,
        EXPIRED_SESSION_SWEEP_INTERVAL,
        EXPIRED_SESSION_SWEEP_BATCH_SIZE,
    ))
}

/// Delete expired records every `period`, forever.
///
/// The first sweep runs at once, which clears records that expired while the server was down.
/// A failed sweep is logged and retried at the next tick, so it never stops the server.
async fn run_expired_session_sweeper(
    store: DatabaseSessionStore,
    period: Duration,
    batch_size: u64,
) {
    let mut interval = tokio::time::interval(period);
    loop {
        interval.tick().await;
        match store.delete_expired_records(batch_size).await {
            Ok(deleted_rows) => tracing::debug!(deleted_rows),
            Err(error) => tracing::error!(error = ?error),
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use std::collections::HashMap;

    use mmcp_db::entities::http_session::Entity;
    use sea_orm::{
        ConnectionTrait, DatabaseConnection, DbBackend, EntityTrait, PaginatorTrait, Statement,
    };
    use time::{Duration as TimeDuration, OffsetDateTime};
    use tower_sessions::SessionStore;
    use tower_sessions::session::{Id, Record};

    use super::*;
    use crate::session_store::test_support::test_connection;

    const SWEEP_PERIOD: Duration = Duration::from_millis(10);
    const SWEEP_BATCH_SIZE: u64 = 2;
    const EXPIRED_RECORDS: usize = 5;
    /// Upper bound on the wait for the sweep, never a pause the assertions depend on.
    const SWEEP_WAIT_LIMIT: Duration = Duration::from_secs(10);
    /// Time the sweeper runs against a missing table: several failing ticks at [`SWEEP_PERIOD`].
    const FAILING_SWEEPS_DURATION: Duration = Duration::from_millis(100);
    const OFFLINE_TABLE: &str = "http_sessions_offline";

    fn record_expiring_in(lifetime: TimeDuration) -> Record {
        Record {
            id: Id::default(),
            data: HashMap::new(),
            expiry_date: OffsetDateTime::now_utc() + lifetime,
        }
    }

    async fn row_count(conn: &DatabaseConnection) -> u64 {
        Entity::find().count(conn).await.unwrap()
    }

    #[tokio::test]
    async fn expired_session_sweeper_deletes_every_expired_row_across_batches() {
        let conn = test_connection().await;
        let store = DatabaseSessionStore::new(conn.clone());
        for _ in 0..EXPIRED_RECORDS {
            store
                .create(&mut record_expiring_in(TimeDuration::minutes(-1)))
                .await
                .unwrap();
        }
        let mut live = record_expiring_in(TimeDuration::minutes(15));
        store.create(&mut live).await.unwrap();
        assert_eq!(row_count(&conn).await, EXPIRED_RECORDS as u64 + 1);

        let sweeper = tokio::spawn(run_expired_session_sweeper(
            store.clone(),
            SWEEP_PERIOD,
            SWEEP_BATCH_SIZE,
        ));
        tokio::time::timeout(SWEEP_WAIT_LIMIT, async {
            while row_count(&conn).await > 1 {
                tokio::time::sleep(SWEEP_PERIOD).await;
            }
        })
        .await
        .expect("the sweep must delete every expired row");
        sweeper.abort();

        assert_eq!(row_count(&conn).await, 1);
        assert!(store.load(&live.id).await.unwrap().is_some());
    }

    /// Renames the sessions table so every sweep statement fails, or restores it.
    async fn set_sessions_table_available(conn: &DatabaseConnection, available: bool) {
        let (from, to) = if available {
            (OFFLINE_TABLE, "http_sessions")
        } else {
            ("http_sessions", OFFLINE_TABLE)
        };
        conn.execute_raw(Statement::from_string(
            DbBackend::Sqlite,
            format!("ALTER TABLE {from} RENAME TO {to}"),
        ))
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn expired_session_sweeper_keeps_running_after_a_failed_sweep() {
        let conn = test_connection().await;
        let store = DatabaseSessionStore::new(conn.clone());
        store
            .create(&mut record_expiring_in(TimeDuration::minutes(-1)))
            .await
            .unwrap();
        let mut live = record_expiring_in(TimeDuration::minutes(15));
        store.create(&mut live).await.unwrap();
        set_sessions_table_available(&conn, false).await;

        let sweeper = tokio::spawn(run_expired_session_sweeper(
            store.clone(),
            SWEEP_PERIOD,
            SWEEP_BATCH_SIZE,
        ));
        tokio::time::sleep(FAILING_SWEEPS_DURATION).await;
        assert!(
            !sweeper.is_finished(),
            "a failed sweep must not end the sweeper task"
        );
        set_sessions_table_available(&conn, true).await;

        tokio::time::timeout(SWEEP_WAIT_LIMIT, async {
            while row_count(&conn).await > 1 {
                tokio::time::sleep(SWEEP_PERIOD).await;
            }
        })
        .await
        .expect("a later tick must sweep the expired row once the table is back");
        sweeper.abort();

        assert!(store.load(&live.id).await.unwrap().is_some());
    }
}
