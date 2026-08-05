//! On-disk SQLite file-reopen compatibility test.
//!
//! Every other integration test in this crate uses `sqlite::memory:`,
//! which never round-trips migration bookkeeping through a real file.
//! This test is the load-bearing safety check for the sea-orm
//! 1.1 -> 2.0 migration (see the mmcp project's decision log,
//! "sea-orm 1.1->2.0.1 migration" entry): it proves that a real
//! on-disk database, migrated once, survives being reopened and
//! re-migrated without re-applying any migration or losing data.
//!
//! A second `migrate()` call against the same file must be a pure
//! no-op: the `seaql_migrations` bookkeeping rows (version +
//! `applied_at`) must come back byte-identical, and every
//! previously-inserted row must still be readable. A changed
//! `applied_at` is the decisive failure signal - it means the
//! migration actually re-ran.

use mmcp_db::connect;
use mmcp_db::entities::group::OwnerKind;
use mmcp_db::repository::{group_repo, user_repo};
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, Statement};
use uuid::Uuid;

/// One row of the `seaql_migrations` bookkeeping table.
#[derive(Debug, Clone, PartialEq, Eq)]
struct MigrationRow {
    version: String,
    applied_at: i64,
}

async fn migration_rows(conn: &DatabaseConnection) -> Vec<MigrationRow> {
    let stmt = Statement::from_string(
        DbBackend::Sqlite,
        "SELECT version, applied_at FROM seaql_migrations ORDER BY version".to_owned(),
    );
    conn.query_all_raw(stmt)
        .await
        .expect("query seaql_migrations")
        .into_iter()
        .map(|row| MigrationRow {
            version: row.try_get("", "version").expect("version column"),
            applied_at: row.try_get("", "applied_at").expect("applied_at column"),
        })
        .collect()
}

/// Builds a `sqlite://` URL for a fresh temp file, creating the file
/// if it does not already exist (`mode=rwc`), with forward slashes so
/// the URL parses correctly on Windows.
fn temp_sqlite_url(file_name: &str) -> (std::path::PathBuf, String) {
    let path = std::env::temp_dir().join(file_name);
    let _ = std::fs::remove_file(&path);
    let url_path = path.to_string_lossy().replace('\\', "/");
    (path, format!("sqlite://{url_path}?mode=rwc"))
}

#[tokio::test]
async fn sqlite_file_reopen_preserves_migration_bookkeeping_and_data() {
    let file_name = format!("mmcp-db-reopen-test-{}.sqlite3", Uuid::now_v7());
    let (db_path, url) = temp_sqlite_url(&file_name);

    let alice_id = Uuid::now_v7();
    let group_id = Uuid::now_v7();

    // --- First open: migrate, insert data, record the bookkeeping
    // baseline. The connection (and its pooled sqlite file handle)
    // drops at the end of this block, closing the file before reopen.
    let baseline_versions = {
        let db = connect(&url).await.expect("connect (first open)");
        db.migrate().await.expect("migrate (first open)");

        user_repo::create(
            db.connection(),
            user_repo::NewUser {
                id: alice_id,
                handle: "alice".into(),
                display_name: Some("Alice".into()),
                password_hash: Some("hash".into()),
                email: Some("alice@example.com".into()),
                created_at: 1_700_000_000_000,
            },
        )
        .await
        .expect("insert user (first open)");

        group_repo::create(
            db.connection(),
            group_repo::NewGroup {
                id: group_id,
                slug: "reopen-test".into(),
                owner_kind: OwnerKind::User,
                owner_id: alice_id,
                display_name: None,
                created_at: 1_700_000_000_001,
            },
        )
        .await
        .expect("insert group (first open)");

        let versions = migration_rows(db.connection()).await;
        assert_eq!(
            versions.len(),
            2,
            "expected exactly the 2 known migrations to be recorded, got {versions:?}"
        );
        let version_names: Vec<&str> = versions.iter().map(|r| r.version.as_str()).collect();
        assert_eq!(version_names, vec!["m0001_initial", "m0002_auth_methods"]);
        versions
    };

    // --- Second open, same file: migrate again. This must be a
    // strict no-op against already-applied migrations.
    let db2 = connect(&url).await.expect("connect (second open)");
    db2.migrate().await.expect("migrate (second open)");

    let versions_after_reopen = migration_rows(db2.connection()).await;
    assert_eq!(
        versions_after_reopen, baseline_versions,
        "re-running migrate() on a reopened file must not re-apply or \
         re-timestamp any migration (applied_at must be unchanged)"
    );

    let user = user_repo::find_by_id(db2.connection(), alice_id)
        .await
        .expect("query user (second open)")
        .expect("user row survives reopen");
    assert_eq!(user.handle, "alice");
    assert_eq!(user.display_name.as_deref(), Some("Alice"));
    assert_eq!(user.email.as_deref(), Some("alice@example.com"));

    let group = group_repo::find_by_id(db2.connection(), group_id)
        .await
        .expect("query group (second open)")
        .expect("group row survives reopen");
    assert_eq!(group.slug, "reopen-test");
    assert_eq!(group.owner_id, alice_id);

    drop(db2);
    let _ = std::fs::remove_file(&db_path);
}
