#![allow(clippy::unwrap_used, clippy::expect_used)]
//! The credential epoch writers on Postgres: the increment with `RETURNING`, the verified-epoch check, the rollback.
//!
//! Run with `--ignored` and `POSTGRES_TEST_URL` naming a disposable database.

use mmcp_db::connect;
use mmcp_db::error::DbError;
use mmcp_db::migration::Migrator;
use mmcp_db::repository::credential_epoch::INITIAL_CREDENTIAL_EPOCH;
use mmcp_db::repository::user_repo::NewUser;
use mmcp_db::repository::{passkey_repo, user_repo};
use sea_orm_migration::MigratorTrait;
use uuid::Uuid;

const POSTGRES_TEST_URL_ENV: &str = "POSTGRES_TEST_URL";

const CREATED_AT: i64 = 1_700_000_000_000;

#[tokio::test]
#[ignore = "needs a disposable Postgres database named by POSTGRES_TEST_URL"]
async fn credential_writers_increment_check_and_roll_back_on_postgres() {
    let url = std::env::var(POSTGRES_TEST_URL_ENV).unwrap_or_else(|_| {
        panic!("{POSTGRES_TEST_URL_ENV} must name a disposable Postgres database")
    });
    let db = connect(&url).await.expect("connect to postgres");
    Migrator::up(db.connection(), None)
        .await
        .expect("apply migrations");
    let conn = db.connection();
    let user = user_repo::create(
        conn,
        NewUser {
            id: Uuid::now_v7(),
            handle: format!("epoch-{}", Uuid::now_v7().simple()),
            display_name: None,
            password_hash: None,
            email: None,
            created_at: CREATED_AT,
        },
    )
    .await
    .expect("create user");
    assert_eq!(user.credential_epoch, INITIAL_CREDENTIAL_EPOCH);

    let first = passkey_repo::create(
        conn,
        Uuid::now_v7(),
        user.id,
        "key".into(),
        "{}".into(),
        CREATED_AT,
        Some(INITIAL_CREDENTIAL_EPOCH),
    )
    .await
    .expect("a session at the current epoch adds a passkey");
    assert_eq!(first.owner.credential_epoch, INITIAL_CREDENTIAL_EPOCH + 1);

    let stale_passkey = Uuid::now_v7();
    let refused = passkey_repo::create(
        conn,
        stale_passkey,
        user.id,
        "key".into(),
        "{}".into(),
        CREATED_AT,
        Some(INITIAL_CREDENTIAL_EPOCH),
    )
    .await;
    assert!(
        matches!(refused, Err(DbError::CredentialEpochChanged { .. })),
        "found {refused:?}"
    );
    assert!(
        passkey_repo::find_by_id(conn, stale_passkey)
            .await
            .expect("query passkey")
            .is_none(),
        "a refused write leaves no row"
    );

    let removed = passkey_repo::delete(conn, first.credential.id)
        .await
        .expect("delete the passkey")
        .expect("the passkey existed");
    assert_eq!(removed.credential_epoch, INITIAL_CREDENTIAL_EPOCH + 2);

    let updated = user_repo::update_profile(conn, user.id, None, Some("hash".into()))
        .await
        .expect("set a password");
    assert_eq!(updated.credential_epoch, INITIAL_CREDENTIAL_EPOCH + 3);
}
