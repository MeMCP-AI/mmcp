#![allow(clippy::unwrap_used, clippy::expect_used)]
//! SQLite connection options `mmcp_db::connect` applies to a file database.
//!
//! The write-ahead log keeps a writer from blocking readers file-wide.
//! The busy timeout bounds how long a writer waits on a contended file.

use mmcp_db::{SQLITE_BUSY_TIMEOUT, connect};
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, Statement};
use uuid::Uuid;

/// Builds a `sqlite://` URL for a fresh temp file path with forward slashes so the URL parses on Windows.
fn temp_sqlite_file(file_name: &str) -> (std::path::PathBuf, String) {
    let path = std::env::temp_dir().join(file_name);
    let _ = std::fs::remove_file(&path);
    let url_path = path.to_string_lossy().replace('\\', "/");
    (path, format!("sqlite://{url_path}?mode=rwc"))
}

async fn pragma_text(conn: &DatabaseConnection, pragma: &str) -> String {
    conn.query_one_raw(Statement::from_string(
        DbBackend::Sqlite,
        format!("PRAGMA {pragma}"),
    ))
    .await
    .expect("run pragma")
    .expect("pragma returns a row")
    .try_get_by_index::<String>(0)
    .expect("pragma text column")
}

async fn pragma_integer(conn: &DatabaseConnection, pragma: &str) -> i64 {
    conn.query_one_raw(Statement::from_string(
        DbBackend::Sqlite,
        format!("PRAGMA {pragma}"),
    ))
    .await
    .expect("run pragma")
    .expect("pragma returns a row")
    .try_get_by_index::<i64>(0)
    .expect("pragma integer column")
}

#[tokio::test]
async fn sqlite_connection_runs_in_wal_mode_with_busy_timeout() {
    let (db_path, url) = temp_sqlite_file(&format!("mmcp-db-options-{}.sqlite3", Uuid::now_v7()));

    let db = connect(&url).await.expect("connect to a file database");

    assert_eq!(pragma_text(db.connection(), "journal_mode").await, "wal");
    assert_eq!(
        pragma_integer(db.connection(), "busy_timeout").await,
        i64::try_from(SQLITE_BUSY_TIMEOUT.as_millis()).expect("busy timeout fits an i64")
    );

    drop(db);
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", db_path.display()));
    }
}
