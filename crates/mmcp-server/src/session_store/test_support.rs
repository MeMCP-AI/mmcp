//! Fixtures shared by the session store unit tests.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use sea_orm::DatabaseConnection;

use crate::session_store::DatabaseSessionStore;

/// Connection to a fresh, migrated in-memory database.
pub(super) async fn test_connection() -> DatabaseConnection {
    let db = mmcp_db::connect(&"sqlite::memory:".parse().expect("a valid database URL"))
        .await
        .expect("connect in-memory sqlite");
    db.migrate().await.expect("run migrations");
    db.into_connection()
}

/// Session store over a fresh, migrated in-memory database.
pub(super) async fn test_store() -> DatabaseSessionStore {
    DatabaseSessionStore::new(test_connection().await)
}
