#![allow(clippy::unwrap_used, clippy::expect_used)]
//! `http_sessions` migration (m0004) and its repository primitives.

use std::collections::HashSet;

use mmcp_db::connect;
use mmcp_db::entities::http_session::{Entity, Model};
use mmcp_db::migration::Migrator;
use mmcp_db::repository::http_session_repo::{
    HttpSessionInsertOutcome, delete, delete_expired_batch, find_unexpired, insert_if_absent,
    upsert,
};
use sea_orm::{
    ConnectionTrait, DatabaseConnection, DbBackend, EntityTrait, PaginatorTrait, Statement,
};
use sea_orm_migration::MigratorTrait;

async fn migrated_connection() -> DatabaseConnection {
    let db = connect("sqlite::memory:")
        .await
        .expect("connect to in-memory sqlite");
    db.migrate().await.expect("run migrations");
    db.into_connection()
}

async fn schema_object_names(conn: &DatabaseConnection, object_type: &str) -> HashSet<String> {
    let stmt = Statement::from_string(
        DbBackend::Sqlite,
        format!("SELECT name FROM sqlite_master WHERE type = '{object_type}'"),
    );
    conn.query_all_raw(stmt)
        .await
        .expect("query sqlite_master")
        .into_iter()
        .map(|row| row.try_get::<String>("", "name").expect("name column"))
        .collect()
}

fn row(key: &str, data: &str, expires_at: i64) -> Model {
    Model {
        session_id_sha256: key.to_owned(),
        data: data.to_owned(),
        expires_at,
    }
}

async fn row_count(conn: &DatabaseConnection) -> u64 {
    Entity::find().count(conn).await.expect("count rows")
}

#[tokio::test]
async fn migration_creates_http_sessions_table_and_expiry_index() {
    let conn = migrated_connection().await;

    let tables = schema_object_names(&conn, "table").await;
    assert!(tables.contains("http_sessions"), "found {tables:?}");
    assert!(
        tables.contains("sessions"),
        "the AI session table must stay untouched, found {tables:?}"
    );
    let indexes = schema_object_names(&conn, "index").await;
    assert!(
        indexes.contains("idx_http_sessions_expires_at"),
        "found {indexes:?}"
    );
}

#[tokio::test]
async fn http_session_migration_down_drops_only_its_own_objects() {
    let conn = migrated_connection().await;

    Migrator::down(&conn, Some(1))
        .await
        .expect("step the last migration down");

    let tables = schema_object_names(&conn, "table").await;
    assert!(!tables.contains("http_sessions"), "found {tables:?}");
    for kept in ["users", "sessions", "oauth_accounts"] {
        assert!(
            tables.contains(kept),
            "{kept} must survive, found {tables:?}"
        );
    }
    let indexes = schema_object_names(&conn, "index").await;
    assert!(
        !indexes.contains("idx_http_sessions_expires_at"),
        "found {indexes:?}"
    );
    assert!(
        indexes.contains("idx_memories_group_id"),
        "m0003 indexes must survive, found {indexes:?}"
    );

    Migrator::up(&conn, None)
        .await
        .expect("re-apply the migration after stepping down");
    assert!(
        schema_object_names(&conn, "table")
            .await
            .contains("http_sessions")
    );
}

#[tokio::test]
async fn insert_if_absent_reports_id_taken_without_overwriting() {
    let conn = migrated_connection().await;

    assert_eq!(
        insert_if_absent(&conn, row("key", "first", 100))
            .await
            .unwrap(),
        HttpSessionInsertOutcome::Inserted
    );
    assert_eq!(
        insert_if_absent(&conn, row("key", "second", 200))
            .await
            .unwrap(),
        HttpSessionInsertOutcome::IdTaken
    );

    let stored = find_unexpired(&conn, "key", 0).await.unwrap().unwrap();
    assert_eq!(stored, row("key", "first", 100));
    assert_eq!(row_count(&conn).await, 1);
}

#[tokio::test]
async fn upsert_inserts_then_overwrites_data_and_expiry_under_one_key() {
    let conn = migrated_connection().await;

    upsert(&conn, row("key", "first", 100)).await.unwrap();
    upsert(&conn, row("key", "second", 200)).await.unwrap();

    let stored = find_unexpired(&conn, "key", 0).await.unwrap().unwrap();
    assert_eq!(stored, row("key", "second", 200));
    assert_eq!(row_count(&conn).await, 1);
}

#[tokio::test]
async fn find_unexpired_returns_none_for_a_row_past_its_expiry() {
    let conn = migrated_connection().await;
    upsert(&conn, row("key", "data", 1_000)).await.unwrap();

    assert!(find_unexpired(&conn, "key", 999).await.unwrap().is_some());
    assert!(
        find_unexpired(&conn, "key", 1_000).await.unwrap().is_none(),
        "an expiry equal to now is already expired"
    );
    assert!(find_unexpired(&conn, "key", 1_001).await.unwrap().is_none());
    assert!(
        find_unexpired(&conn, "other", 0).await.unwrap().is_none(),
        "an unknown key finds nothing"
    );
}

#[tokio::test]
async fn delete_removes_the_row_and_tolerates_an_absent_key() {
    let conn = migrated_connection().await;
    upsert(&conn, row("key", "data", 1_000)).await.unwrap();

    delete(&conn, "key").await.unwrap();
    delete(&conn, "key").await.unwrap();

    assert_eq!(row_count(&conn).await, 0);
}

#[tokio::test]
async fn delete_expired_batch_deletes_at_most_the_batch_size_and_only_expired_rows() {
    const EXPIRED_ROWS: u64 = 7;
    const BATCH_SIZE: u64 = 3;
    let conn = migrated_connection().await;
    for index in 0..EXPIRED_ROWS {
        upsert(&conn, row(&format!("expired-{index}"), "data", 100))
            .await
            .unwrap();
    }
    upsert(&conn, row("live", "data", 10_000)).await.unwrap();
    let now = 5_000;

    let first = delete_expired_batch(&conn, now, BATCH_SIZE).await.unwrap();
    assert_eq!(first, BATCH_SIZE);
    assert_eq!(row_count(&conn).await, EXPIRED_ROWS + 1 - BATCH_SIZE);

    let second = delete_expired_batch(&conn, now, BATCH_SIZE).await.unwrap();
    assert_eq!(second, BATCH_SIZE);
    let third = delete_expired_batch(&conn, now, BATCH_SIZE).await.unwrap();
    assert_eq!(third, EXPIRED_ROWS - 2 * BATCH_SIZE);

    assert_eq!(
        delete_expired_batch(&conn, now, BATCH_SIZE).await.unwrap(),
        0,
        "nothing expired remains"
    );
    assert!(
        find_unexpired(&conn, "live", now).await.unwrap().is_some(),
        "a live row is never deleted"
    );
    assert_eq!(row_count(&conn).await, 1);
}
