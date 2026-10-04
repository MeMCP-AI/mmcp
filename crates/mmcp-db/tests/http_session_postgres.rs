#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Concurrent behaviour of the expiry sweep on Postgres, which SQLite cannot show.
//!
//! Run with `--ignored` and `POSTGRES_TEST_URL` naming a disposable database.

use std::time::Duration;

use mmcp_db::connect;
use mmcp_db::entities::http_session::Model;
use mmcp_db::migration::Migrator;
use mmcp_db::repository::http_session_repo::{delete_expired_batch, find_unexpired, upsert};
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, Statement, TransactionTrait};
use sea_orm_migration::MigratorTrait;
use uuid::Uuid;

const POSTGRES_TEST_URL_ENV: &str = "POSTGRES_TEST_URL";

/// Upper bound on the wait for the sweep to block on the row lock, never a pause an assertion depends on.
const LOCK_WAIT_LIMIT: Duration = Duration::from_secs(10);

const LOCK_POLL_INTERVAL: Duration = Duration::from_millis(25);

const SWEEP_BATCH_SIZE: u64 = 100;

const NOW: i64 = 1_000_000;

const EXTENDED_EXPIRY: i64 = 9_000_000;

async fn sweep_is_blocked_on_a_row_lock(conn: &DatabaseConnection) -> bool {
    let row = conn
        .query_one_raw(Statement::from_string(
            DbBackend::Postgres,
            "SELECT count(*) AS blocked FROM pg_stat_activity \
             WHERE wait_event_type = 'Lock' AND query LIKE 'DELETE FROM \"http_sessions\"%'"
                .to_owned(),
        ))
        .await
        .expect("query pg_stat_activity")
        .expect("one row");
    row.try_get::<i64>("", "blocked").expect("count column") > 0
}

#[tokio::test]
#[ignore = "needs a disposable Postgres database named by POSTGRES_TEST_URL"]
async fn sweep_keeps_a_session_extended_while_the_sweep_waits_on_its_row() {
    let url = std::env::var(POSTGRES_TEST_URL_ENV).unwrap_or_else(|_| {
        panic!("{POSTGRES_TEST_URL_ENV} must name a disposable Postgres database")
    });
    let db = connect(&url).await.expect("connect to postgres");
    Migrator::up(db.connection(), None)
        .await
        .expect("apply migrations");
    let conn = db.connection().clone();
    let key = format!("sweep-race-{}", Uuid::now_v7().simple());
    upsert(
        &conn,
        Model {
            session_id_sha256: key.clone(),
            data: "{}".to_owned(),
            expires_at: NOW - 1,
        },
    )
    .await
    .expect("seed an expired row");

    // A request extends the session: its update holds the row lock until it commits.
    let extending = conn.begin().await.expect("begin the extending transaction");
    extending
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE http_sessions SET expires_at = $1 WHERE session_id_sha256 = $2",
            [EXTENDED_EXPIRY.into(), key.clone().into()],
        ))
        .await
        .expect("extend the session");

    // The sweep computes its target set from the committed (expired) version, then waits on the lock.
    let sweeping_conn = conn.clone();
    let sweep =
        tokio::spawn(
            async move { delete_expired_batch(&sweeping_conn, NOW, SWEEP_BATCH_SIZE).await },
        );
    tokio::time::timeout(LOCK_WAIT_LIMIT, async {
        while !sweep_is_blocked_on_a_row_lock(&conn).await {
            tokio::time::sleep(LOCK_POLL_INTERVAL).await;
        }
    })
    .await
    .expect("the sweep must block on the row the request is extending");

    extending.commit().await.expect("commit the extension");
    sweep.await.expect("sweep task").expect("sweep statement");

    assert!(
        find_unexpired(&conn, &key, NOW)
            .await
            .expect("read the row")
            .is_some(),
        "a session extended at the same instant must survive the sweep"
    );
}
