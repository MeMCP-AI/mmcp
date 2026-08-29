#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Proves the m0003 index migration actually creates every index and
//! that SQLite's query planner picks each one up instead of falling
//! back to a full table scan.

use mmcp_db::connect;
use mmcp_db::migration::Migrator;
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, Statement};
use sea_orm_migration::MigratorTrait;
use std::collections::HashSet;
use uuid::Uuid;

async fn fresh_database() -> DatabaseConnection {
    let db = connect("sqlite::memory:")
        .await
        .expect("connect to in-memory sqlite");
    db.migrate().await.expect("run migrations");
    db.into_connection()
}

/// Every index name the m0003 migration is expected to create, on
/// top of the one pre-existing index from m0002
/// (`idx_oauth_provider_user`).
const EXPECTED_NEW_INDEXES: &[&str] = &[
    "idx_memories_group_id",
    "idx_memory_versions_memory_id",
    "idx_groups_owner",
    "idx_group_memberships_group_id",
    "idx_org_members_org_id",
    "idx_memory_reads_session_id",
    "idx_memory_reads_memory_id",
    "idx_passkey_credentials_user_id",
    "idx_oauth_accounts_user_id",
];

async fn index_names(conn: &DatabaseConnection) -> HashSet<String> {
    let stmt = Statement::from_string(
        DbBackend::Sqlite,
        "SELECT name FROM sqlite_master WHERE type = 'index'".to_owned(),
    );
    let rows = conn.query_all_raw(stmt).await.expect("query sqlite_master");
    rows.into_iter()
        .map(|row| row.try_get::<String>("", "name").expect("name column"))
        .collect()
}

/// Runs `EXPLAIN QUERY PLAN` for `sql` and returns the concatenated
/// `detail` column of every plan row (SQLite's plan text lives there
/// regardless of the 3- vs 4-column `EXPLAIN QUERY PLAN` shape used
/// by the linked SQLite version).
async fn query_plan(conn: &DatabaseConnection, sql: &str) -> String {
    let stmt = Statement::from_string(DbBackend::Sqlite, format!("EXPLAIN QUERY PLAN {sql}"));
    let rows = conn.query_all_raw(stmt).await.expect("explain query plan");
    rows.into_iter()
        .map(|row| row.try_get::<String>("", "detail").expect("detail column"))
        .collect::<Vec<_>>()
        .join(" | ")
}

#[tokio::test]
async fn migration_creates_every_expected_index() {
    let conn = fresh_database().await;
    let names = index_names(&conn).await;

    for expected in EXPECTED_NEW_INDEXES {
        assert!(
            names.contains(*expected),
            "expected index {expected} to exist, found {names:?}"
        );
    }
    // The one index that predates this migration must still be there.
    assert!(names.contains("idx_oauth_provider_user"));
}

/// The production path is a database that already has rows when
/// m0003 runs, not a fresh one: apply only m0001+m0002, insert a
/// row, then run m0003 and confirm both the index and the
/// pre-existing row survive.
#[tokio::test]
async fn migration_builds_indexes_over_a_database_with_existing_data() {
    let db = connect("sqlite::memory:")
        .await
        .expect("connect to in-memory sqlite");
    let conn = db.connection();
    Migrator::up(conn, Some(2))
        .await
        .expect("apply m0001+m0002 only");

    let user_id = Uuid::now_v7();
    mmcp_db::repository::user_repo::create(
        conn,
        mmcp_db::repository::user_repo::NewUser {
            id: user_id,
            handle: "pre-existing".into(),
            display_name: None,
            password_hash: None,
            email: None,
            created_at: 1,
        },
    )
    .await
    .expect("insert row before m0003 runs");

    Migrator::up(conn, None)
        .await
        .expect("apply remaining migrations, including m0003");

    let names = index_names(conn).await;
    for expected in EXPECTED_NEW_INDEXES {
        assert!(
            names.contains(*expected),
            "expected index {expected} to exist after migrating over existing data, found {names:?}"
        );
    }

    let row = mmcp_db::repository::user_repo::find_by_id(conn, user_id)
        .await
        .expect("query user")
        .expect("pre-existing row survives m0003");
    assert_eq!(row.handle, "pre-existing");
}

#[tokio::test]
async fn memories_group_id_filter_uses_index_not_full_scan() {
    let conn = fresh_database().await;
    let plan = query_plan(&conn, "SELECT * FROM memories WHERE group_id = 'g'").await;
    assert!(
        plan.contains("USING INDEX idx_memories_group_id"),
        "expected the group_id filter to use idx_memories_group_id, got: {plan}"
    );
}

#[tokio::test]
async fn memory_versions_memory_id_filter_uses_index_not_full_scan() {
    let conn = fresh_database().await;
    let plan = query_plan(&conn, "SELECT * FROM memory_versions WHERE memory_id = 'm'").await;
    assert!(
        plan.contains("USING INDEX idx_memory_versions_memory_id"),
        "expected the memory_id filter to use idx_memory_versions_memory_id, got: {plan}"
    );
}

#[tokio::test]
async fn memory_reads_session_and_memory_filter_uses_index_not_full_scan() {
    let conn = fresh_database().await;
    let plan = query_plan(
        &conn,
        "SELECT * FROM memory_reads WHERE session_id = 's' AND memory_id = 'm'",
    )
    .await;
    assert!(
        plan.contains("USING INDEX"),
        "expected the session_id/memory_id filter to use an index, got: {plan}"
    );
}

#[tokio::test]
async fn groups_owner_filter_uses_index_not_full_scan() {
    let conn = fresh_database().await;
    let plan = query_plan(
        &conn,
        "SELECT * FROM groups WHERE owner_kind = 1 AND owner_id = 'o'",
    )
    .await;
    assert!(
        plan.contains("USING INDEX idx_groups_owner"),
        "expected the owner_kind/owner_id filter to use idx_groups_owner, got: {plan}"
    );
}

#[tokio::test]
async fn oauth_accounts_user_id_filter_uses_index_not_full_scan() {
    let conn = fresh_database().await;
    let plan = query_plan(&conn, "SELECT * FROM oauth_accounts WHERE user_id = 'u'").await;
    assert!(
        plan.contains("USING INDEX idx_oauth_accounts_user_id"),
        "expected the user_id filter to use idx_oauth_accounts_user_id, got: {plan}"
    );
}

#[tokio::test]
async fn passkey_credentials_user_id_filter_uses_index_not_full_scan() {
    let conn = fresh_database().await;
    let plan = query_plan(
        &conn,
        "SELECT * FROM passkey_credentials WHERE user_id = 'u'",
    )
    .await;
    assert!(
        plan.contains("USING INDEX idx_passkey_credentials_user_id"),
        "expected the user_id filter to use idx_passkey_credentials_user_id, got: {plan}"
    );
}
