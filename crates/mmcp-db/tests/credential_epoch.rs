#![allow(clippy::unwrap_used, clippy::expect_used)]
//! The credential epoch (m0005): which writes increment it, and what each writer returns.

use mmcp_db::connect;
use mmcp_db::entities::oauth_account;
use mmcp_db::error::DbError;
use mmcp_db::migration::Migrator;
use mmcp_db::repository::credential_epoch::INITIAL_CREDENTIAL_EPOCH;
use mmcp_db::repository::oauth_repo::NewOauthAccount;
use mmcp_db::repository::user_repo::NewUser;
use mmcp_db::repository::{oauth_repo, passkey_repo, user_repo};
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, Statement};
use sea_orm_migration::MigratorTrait;
use uuid::Uuid;

const CREATED_AT: i64 = 1_700_000_000_000;

async fn migrated_connection() -> DatabaseConnection {
    let db = connect("sqlite::memory:")
        .await
        .expect("connect to in-memory sqlite");
    db.migrate().await.expect("run migrations");
    db.into_connection()
}

fn new_user(handle: &str) -> NewUser {
    NewUser {
        id: Uuid::now_v7(),
        handle: handle.to_owned(),
        display_name: None,
        password_hash: None,
        email: None,
        created_at: CREATED_AT,
    }
}

async fn user_with_epoch_zero(conn: &DatabaseConnection, handle: &str) -> Uuid {
    user_repo::create(conn, new_user(handle))
        .await
        .expect("create user")
        .id
}

async fn stored_epoch(conn: &DatabaseConnection, user_id: Uuid) -> i64 {
    user_repo::require(conn, user_id)
        .await
        .expect("load user")
        .credential_epoch
}

async fn add_passkey(conn: &DatabaseConnection, user_id: Uuid) -> Uuid {
    let id = Uuid::now_v7();
    passkey_repo::create(conn, id, user_id, "key".into(), "{}".into(), CREATED_AT)
        .await
        .expect("create passkey");
    id
}

fn oauth_link(user_id: Uuid, provider_user_id: &str) -> NewOauthAccount {
    NewOauthAccount {
        id: Uuid::now_v7(),
        user_id,
        provider: "github".into(),
        provider_user_id: provider_user_id.to_owned(),
        email: None,
        access_token: None,
        refresh_token: None,
        created_at: CREATED_AT,
    }
}

async fn column_names(conn: &DatabaseConnection, table: &str) -> Vec<String> {
    let stmt = Statement::from_string(DbBackend::Sqlite, format!("PRAGMA table_info({table})"));
    conn.query_all_raw(stmt)
        .await
        .expect("query table_info")
        .into_iter()
        .map(|row| row.try_get::<String>("", "name").expect("name column"))
        .collect()
}

#[tokio::test]
async fn new_user_starts_at_initial_epoch() {
    let conn = migrated_connection().await;

    let user = user_repo::create(&conn, new_user("alice"))
        .await
        .expect("create user");

    assert_eq!(user.credential_epoch, INITIAL_CREDENTIAL_EPOCH);
    assert_eq!(stored_epoch(&conn, user.id).await, INITIAL_CREDENTIAL_EPOCH);
}

#[tokio::test]
async fn passkey_create_increments_the_epoch_by_one() {
    let conn = migrated_connection().await;
    let user_id = user_with_epoch_zero(&conn, "alice").await;

    add_passkey(&conn, user_id).await;

    assert_eq!(
        stored_epoch(&conn, user_id).await,
        INITIAL_CREDENTIAL_EPOCH + 1
    );
}

#[tokio::test]
async fn passkey_delete_increments_the_epoch_by_one() {
    let conn = migrated_connection().await;
    let user_id = user_with_epoch_zero(&conn, "alice").await;
    let passkey_id = add_passkey(&conn, user_id).await;
    let before = stored_epoch(&conn, user_id).await;

    passkey_repo::delete(&conn, passkey_id)
        .await
        .expect("delete passkey");

    assert_eq!(stored_epoch(&conn, user_id).await, before + 1);
}

#[tokio::test]
async fn passkey_delete_of_a_missing_row_changes_no_epoch() {
    let conn = migrated_connection().await;
    let user_id = user_with_epoch_zero(&conn, "alice").await;

    let owner = passkey_repo::delete(&conn, Uuid::now_v7())
        .await
        .expect("delete of a missing passkey is not an error");

    assert!(owner.is_none());
    assert_eq!(stored_epoch(&conn, user_id).await, INITIAL_CREDENTIAL_EPOCH);
}

#[tokio::test]
async fn passkey_delete_of_an_already_deleted_row_changes_no_epoch() {
    let conn = migrated_connection().await;
    let user_id = user_with_epoch_zero(&conn, "alice").await;
    let passkey_id = add_passkey(&conn, user_id).await;
    passkey_repo::delete(&conn, passkey_id)
        .await
        .expect("first delete");
    let after_first_delete = stored_epoch(&conn, user_id).await;

    let owner = passkey_repo::delete(&conn, passkey_id)
        .await
        .expect("second delete");

    assert!(owner.is_none());
    assert_eq!(stored_epoch(&conn, user_id).await, after_first_delete);
}

#[tokio::test]
async fn passkey_update_after_auth_keeps_the_epoch() {
    let conn = migrated_connection().await;
    let user_id = user_with_epoch_zero(&conn, "alice").await;
    let passkey_id = add_passkey(&conn, user_id).await;
    let before = stored_epoch(&conn, user_id).await;

    passkey_repo::update_after_auth(&conn, passkey_id, "{\"counter\":2}".into(), CREATED_AT + 1)
        .await
        .expect("update after auth");

    assert_eq!(stored_epoch(&conn, user_id).await, before);
}

#[tokio::test]
async fn oauth_link_create_increments_the_epoch_by_one() {
    let conn = migrated_connection().await;
    let user_id = user_with_epoch_zero(&conn, "alice").await;

    oauth_repo::create(&conn, oauth_link(user_id, "gh-1"))
        .await
        .expect("link oauth account");

    assert_eq!(
        stored_epoch(&conn, user_id).await,
        INITIAL_CREDENTIAL_EPOCH + 1
    );
}

#[tokio::test]
async fn oauth_update_tokens_keeps_the_epoch() {
    let conn = migrated_connection().await;
    let user_id = user_with_epoch_zero(&conn, "alice").await;
    let link = oauth_repo::create(&conn, oauth_link(user_id, "gh-1"))
        .await
        .expect("link oauth account")
        .credential;
    let before = stored_epoch(&conn, user_id).await;

    oauth_repo::update_tokens(
        &conn,
        link.id,
        Some("a".into()),
        Some("r".into()),
        CREATED_AT + 1,
    )
    .await
    .expect("update tokens");

    assert_eq!(stored_epoch(&conn, user_id).await, before);
}

#[tokio::test]
async fn update_profile_with_password_increments_the_epoch_by_one() {
    let conn = migrated_connection().await;
    let user_id = user_with_epoch_zero(&conn, "alice").await;

    user_repo::update_profile(&conn, user_id, Some("Alice".into()), Some("hash".into()))
        .await
        .expect("update profile");

    assert_eq!(
        stored_epoch(&conn, user_id).await,
        INITIAL_CREDENTIAL_EPOCH + 1
    );
}

#[tokio::test]
async fn update_profile_without_password_keeps_the_epoch() {
    let conn = migrated_connection().await;
    let user_id = user_with_epoch_zero(&conn, "alice").await;

    let updated = user_repo::update_profile(&conn, user_id, Some("Alice".into()), None)
        .await
        .expect("update profile");

    assert_eq!(updated.display_name.as_deref(), Some("Alice"));
    assert_eq!(stored_epoch(&conn, user_id).await, INITIAL_CREDENTIAL_EPOCH);
}

#[tokio::test]
async fn passkey_create_for_a_missing_user_leaves_no_passkey_row() {
    let conn = migrated_connection().await;
    let missing_user = Uuid::now_v7();
    let passkey_id = Uuid::now_v7();

    let outcome = passkey_repo::create(
        &conn,
        passkey_id,
        missing_user,
        "key".into(),
        "{}".into(),
        CREATED_AT,
    )
    .await;

    assert!(
        matches!(outcome, Err(DbError::CredentialOwnerMissing { user_id }) if user_id == missing_user),
        "found {outcome:?}"
    );
    assert!(
        passkey_repo::find_by_id(&conn, passkey_id)
            .await
            .expect("query passkey")
            .is_none()
    );
}

#[tokio::test]
async fn credential_writers_return_the_epoch_they_committed() {
    let conn = migrated_connection().await;
    let user_id = user_with_epoch_zero(&conn, "alice").await;

    let passkey_id = Uuid::now_v7();
    let passkey_write = passkey_repo::create(
        &conn,
        passkey_id,
        user_id,
        "key".into(),
        "{}".into(),
        CREATED_AT,
    )
    .await
    .expect("create passkey");
    assert_eq!(
        passkey_write.owner.credential_epoch,
        stored_epoch(&conn, user_id).await
    );

    let oauth_write = oauth_repo::create(&conn, oauth_link(user_id, "gh-1"))
        .await
        .expect("link oauth account");
    assert_eq!(
        oauth_write.owner.credential_epoch,
        stored_epoch(&conn, user_id).await
    );
    assert_eq!(oauth_write.owner.id, user_id);

    let profile = user_repo::update_profile(&conn, user_id, None, Some("hash".into()))
        .await
        .expect("update profile");
    assert_eq!(profile.credential_epoch, stored_epoch(&conn, user_id).await);
    assert_eq!(profile.password_hash.as_deref(), Some("hash"));

    let removed_owner = passkey_repo::delete(&conn, passkey_id)
        .await
        .expect("delete passkey")
        .expect("the passkey existed");
    assert_eq!(
        removed_owner.credential_epoch,
        stored_epoch(&conn, user_id).await
    );
    assert_eq!(
        removed_owner.credential_epoch,
        INITIAL_CREDENTIAL_EPOCH + 4,
        "four credential changes happened"
    );
}

#[tokio::test]
async fn create_with_oauth_link_writes_user_and_link_at_the_initial_epoch() {
    let conn = migrated_connection().await;
    let new = new_user("alice");
    let user_id = new.id;

    let (user, link) =
        user_repo::create_with_oauth_link(&conn, new, oauth_link(Uuid::nil(), "gh-1"))
            .await
            .expect("create user with link");

    assert_eq!(user.credential_epoch, INITIAL_CREDENTIAL_EPOCH);
    assert_eq!(stored_epoch(&conn, user_id).await, INITIAL_CREDENTIAL_EPOCH);
    assert_eq!(
        link.user_id, user_id,
        "the link belongs to the created user"
    );
    let stored_link: Option<oauth_account::Model> =
        oauth_repo::find_by_provider(&conn, "github", "gh-1")
            .await
            .expect("query link");
    assert_eq!(stored_link.expect("link row").user_id, user_id);
}

#[tokio::test]
async fn create_with_oauth_link_leaves_no_user_when_the_link_insert_fails() {
    let conn = migrated_connection().await;
    let first = new_user("alice");
    user_repo::create_with_oauth_link(&conn, first, oauth_link(Uuid::nil(), "gh-1"))
        .await
        .expect("first login");
    let second = new_user("bob");
    let second_id = second.id;

    let outcome =
        user_repo::create_with_oauth_link(&conn, second, oauth_link(Uuid::nil(), "gh-1")).await;

    assert!(outcome.is_err(), "the provider account is already linked");
    assert!(
        user_repo::find_by_id(&conn, second_id)
            .await
            .expect("query user")
            .is_none(),
        "the failed first login must not leave its user behind"
    );
    assert!(
        user_repo::find_by_handle(&conn, "bob")
            .await
            .expect("query handle")
            .is_none()
    );
}

#[tokio::test]
async fn credential_epoch_migration_defaults_existing_users_to_zero() {
    let conn = migrated_connection().await;
    Migrator::down(&conn, Some(1))
        .await
        .expect("step the epoch migration down");
    let legacy_id = Uuid::now_v7();
    conn.execute_raw(Statement::from_sql_and_values(
        DbBackend::Sqlite,
        "INSERT INTO users (id, handle, display_name, password_hash, email, created_at) \
         VALUES (?, ?, NULL, NULL, NULL, ?)",
        [legacy_id.into(), "legacy".into(), CREATED_AT.into()],
    ))
    .await
    .expect("insert a user that predates the epoch");

    Migrator::up(&conn, None)
        .await
        .expect("apply the epoch migration");

    assert_eq!(stored_epoch(&conn, legacy_id).await, 0);
}

#[tokio::test]
async fn credential_epoch_migration_down_drops_only_its_column() {
    let conn = migrated_connection().await;
    assert!(
        column_names(&conn, "users")
            .await
            .contains(&"credential_epoch".to_owned())
    );

    Migrator::down(&conn, Some(1))
        .await
        .expect("step the epoch migration down");

    let columns = column_names(&conn, "users").await;
    assert!(
        !columns.contains(&"credential_epoch".to_owned()),
        "{columns:?}"
    );
    for kept in [
        "id",
        "handle",
        "display_name",
        "password_hash",
        "email",
        "created_at",
    ] {
        assert!(columns.contains(&kept.to_owned()), "{kept} in {columns:?}");
    }
    assert!(!column_names(&conn, "http_sessions").await.is_empty());
}
