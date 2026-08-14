//! End-to-end smoke test against an in-memory SQLite database.

use mmcp_db::entities::group::OwnerKind;
use mmcp_db::entities::membership::{GroupRole, PrincipalKind};
use mmcp_db::entities::memory::MemoryKind;
use mmcp_db::entities::{membership, memory_read, memory_version};
use mmcp_db::repository::{
    group_repo, memory_repo, oauth_repo, org_repo, passkey_repo, session_repo, user_repo,
};
use mmcp_db::{Database, connect};
use uuid::Uuid;

async fn fresh_database() -> Database {
    let db = connect("sqlite::memory:")
        .await
        .expect("connect to in-memory sqlite");
    db.migrate().await.expect("run migrations");
    db
}

#[tokio::test]
async fn migrate_creates_all_tables() {
    let db = fresh_database().await;
    // The next step works only if the tables exist.
    let users = user_repo::find_by_handle(db.connection(), "nobody")
        .await
        .expect("query users table");
    assert!(users.is_none());
}

#[tokio::test]
async fn user_create_and_lookup_roundtrip() {
    let db = fresh_database().await;
    let user = user_repo::create(
        db.connection(),
        user_repo::NewUser {
            id: Uuid::now_v7(),
            handle: "alice".into(),
            display_name: Some("Alice".into()),
            password_hash: Some("hash".into()),
            email: Some("alice@example.com".into()),
            created_at: 1_700_000_000_000,
        },
    )
    .await
    .expect("create alice");

    let fetched = user_repo::find_by_handle(db.connection(), "alice")
        .await
        .expect("fetch alice")
        .expect("alice present");
    assert_eq!(fetched.id, user.id);
    assert_eq!(fetched.display_name.as_deref(), Some("Alice"));
}

#[tokio::test]
async fn org_memberships_and_groups_cascade() {
    let db = fresh_database().await;
    let conn = db.connection();

    let alice_id = Uuid::now_v7();
    user_repo::create(
        conn,
        user_repo::NewUser {
            id: alice_id,
            handle: "alice".into(),
            display_name: None,
            password_hash: None,
            email: None,
            created_at: 1,
        },
    )
    .await
    .unwrap();

    let org_id = Uuid::now_v7();
    org_repo::create(
        conn,
        org_repo::NewOrg {
            id: org_id,
            slug: "acme".into(),
            display_name: None,
            created_at: 2,
        },
    )
    .await
    .unwrap();

    let group_id = Uuid::now_v7();
    group_repo::create(
        conn,
        group_repo::NewGroup {
            id: group_id,
            slug: "shared".into(),
            owner_kind: OwnerKind::Org,
            owner_id: org_id,
            display_name: None,
            created_at: 3,
        },
    )
    .await
    .unwrap();

    group_repo::add_membership(
        conn,
        membership::Model {
            id: Uuid::now_v7(),
            group_id,
            principal_kind: PrincipalKind::User,
            principal_id: alice_id,
            role: GroupRole::Write,
            granted_at: 4,
        },
    )
    .await
    .unwrap();

    let memberships = group_repo::list_memberships(conn, group_id).await.unwrap();
    assert_eq!(memberships.len(), 1);
    assert_eq!(memberships[0].principal_id, alice_id);
    assert_eq!(memberships[0].role, GroupRole::Write);
}

#[tokio::test]
async fn memory_versions_and_latest_version_update() {
    let db = fresh_database().await;
    let conn = db.connection();

    let alice = Uuid::now_v7();
    user_repo::create(
        conn,
        user_repo::NewUser {
            id: alice,
            handle: "alice".into(),
            display_name: None,
            password_hash: None,
            email: None,
            created_at: 1,
        },
    )
    .await
    .unwrap();

    let group_id = Uuid::now_v7();
    group_repo::create(
        conn,
        group_repo::NewGroup {
            id: group_id,
            slug: "g".into(),
            owner_kind: OwnerKind::User,
            owner_id: alice,
            display_name: None,
            created_at: 1,
        },
    )
    .await
    .unwrap();

    let memory_id = Uuid::now_v7();
    memory_repo::create(
        conn,
        memory_repo::NewMemory {
            id: memory_id,
            group_id,
            slug: "rules".into(),
            kind: MemoryKind::Rule,
            mandatory: true,
            created_at: 1,
            updated_at: 1,
        },
    )
    .await
    .unwrap();

    memory_repo::record_version(
        conn,
        memory_version::Model {
            id: Uuid::now_v7(),
            memory_id,
            version: "0.1.0".into(),
            commit: "abc123".into(),
            author_id: alice,
            published_at: 2,
            summary: Some("initial".into()),
        },
    )
    .await
    .unwrap();

    let updated = memory_repo::set_latest_version(conn, memory_id, "0.1.0".into(), 2)
        .await
        .unwrap();
    assert_eq!(updated.latest_version.as_deref(), Some("0.1.0"));

    let history = memory_repo::list_versions(conn, memory_id).await.unwrap();
    assert_eq!(history.len(), 1);
}

#[tokio::test]
async fn session_turn_counter_increments() {
    let db = fresh_database().await;
    let conn = db.connection();

    session_repo::upsert(
        conn,
        session_repo::NewSession {
            session_id: "sess-1".into(),
            user_id: None,
            project_uuid: None,
            transcript_path: None,
            started_at: 1,
        },
    )
    .await
    .unwrap();

    let first = session_repo::bump_turn(conn, "sess-1", 2).await.unwrap();
    let second = session_repo::bump_turn(conn, "sess-1", 3).await.unwrap();
    assert_eq!(first, 1);
    assert_eq!(second, 2);
}

#[tokio::test]
async fn memory_read_tracking_and_post_compaction() {
    let db = fresh_database().await;
    let conn = db.connection();

    let alice = Uuid::now_v7();
    user_repo::create(
        conn,
        user_repo::NewUser {
            id: alice,
            handle: "alice".into(),
            display_name: None,
            password_hash: None,
            email: None,
            created_at: 1,
        },
    )
    .await
    .unwrap();

    let group_id = Uuid::now_v7();
    group_repo::create(
        conn,
        group_repo::NewGroup {
            id: group_id,
            slug: "g".into(),
            owner_kind: OwnerKind::User,
            owner_id: alice,
            display_name: None,
            created_at: 1,
        },
    )
    .await
    .unwrap();

    let memory_id = Uuid::now_v7();
    memory_repo::create(
        conn,
        memory_repo::NewMemory {
            id: memory_id,
            group_id,
            slug: "rules".into(),
            kind: MemoryKind::Rule,
            mandatory: true,
            created_at: 1,
            updated_at: 1,
        },
    )
    .await
    .unwrap();

    session_repo::upsert(
        conn,
        session_repo::NewSession {
            session_id: "sess-1".into(),
            user_id: Some(alice),
            project_uuid: None,
            transcript_path: None,
            started_at: 1,
        },
    )
    .await
    .unwrap();

    assert!(
        !session_repo::has_read(conn, "sess-1", memory_id)
            .await
            .unwrap()
    );

    session_repo::record_read(
        conn,
        memory_read::Model {
            id: Uuid::now_v7(),
            session_id: "sess-1".into(),
            memory_id,
            turn: 1,
            version: Some("0.1.0".into()),
            verified: false,
            read_at: 2,
        },
    )
    .await
    .unwrap();

    assert!(
        session_repo::has_read(conn, "sess-1", memory_id)
            .await
            .unwrap()
    );

    session_repo::mark_post_compaction(conn, "sess-1", Some("sig".into()), 3)
        .await
        .unwrap();
    let refreshed = session_repo::find(conn, "sess-1").await.unwrap().unwrap();
    assert!(refreshed.post_compaction);

    session_repo::clear_post_compaction(conn, "sess-1")
        .await
        .unwrap();
    let refreshed = session_repo::find(conn, "sess-1").await.unwrap().unwrap();
    assert!(!refreshed.post_compaction);
}

#[tokio::test]
async fn oauth_account_create_find_and_update_tokens_round_trip() {
    let db = fresh_database().await;
    let conn = db.connection();

    // OAuth rows foreign-key onto users; create one first.
    let alice = Uuid::now_v7();
    user_repo::create(
        conn,
        user_repo::NewUser {
            id: alice,
            handle: "alice".into(),
            display_name: None,
            password_hash: None,
            email: Some("alice@example.com".into()),
            created_at: 1,
        },
    )
    .await
    .unwrap();

    let link_id = Uuid::now_v7();
    oauth_repo::create(
        conn,
        oauth_repo::NewOauthAccount {
            id: link_id,
            user_id: alice,
            provider: "github".into(),
            provider_user_id: "octocat".into(),
            email: Some("alice@example.com".into()),
            access_token: Some("access-1".into()),
            refresh_token: Some("refresh-1".into()),
            created_at: 10,
        },
    )
    .await
    .expect("insert github link");

    // Lookup by (provider, provider_user_id): the login flow's path.
    let by_provider = oauth_repo::find_by_provider(conn, "github", "octocat")
        .await
        .expect("query")
        .expect("row present");
    assert_eq!(by_provider.id, link_id);
    assert_eq!(by_provider.user_id, alice);
    assert_eq!(by_provider.access_token.as_deref(), Some("access-1"));

    // Unknown (provider, provider_user_id) returns None: the first-time-signup path.
    let missing = oauth_repo::find_by_provider(conn, "github", "nobody")
        .await
        .unwrap();
    assert!(missing.is_none());

    // All links for a user.
    let user_links = oauth_repo::find_by_user(conn, alice).await.unwrap();
    assert_eq!(user_links.len(), 1);

    // Token refresh path advances both tokens and updated_at.
    let refreshed = oauth_repo::update_tokens(
        conn,
        link_id,
        Some("access-2".into()),
        Some("refresh-2".into()),
        20,
    )
    .await
    .expect("update tokens");
    assert_eq!(refreshed.access_token.as_deref(), Some("access-2"));
    assert_eq!(refreshed.refresh_token.as_deref(), Some("refresh-2"));
    assert_eq!(refreshed.updated_at, 20);

    // Updating a non-existent link surfaces NotFound.
    let missing_update = oauth_repo::update_tokens(conn, Uuid::now_v7(), None, None, 30).await;
    assert!(matches!(missing_update, Err(mmcp_db::DbError::NotFound)));
}

#[tokio::test]
async fn passkey_credential_create_find_update_and_delete_round_trip() {
    let db = fresh_database().await;
    let conn = db.connection();

    let bob = Uuid::now_v7();
    user_repo::create(
        conn,
        user_repo::NewUser {
            id: bob,
            handle: "bob".into(),
            display_name: None,
            password_hash: None,
            email: None,
            created_at: 1,
        },
    )
    .await
    .unwrap();

    let cred_id = Uuid::now_v7();
    passkey_repo::create(
        conn,
        cred_id,
        bob,
        "laptop".into(),
        r#"{"counter":0}"#.into(),
        10,
    )
    .await
    .expect("insert passkey");

    // Found by id.
    let fetched = passkey_repo::find_by_id(conn, cred_id)
        .await
        .unwrap()
        .expect("present");
    assert_eq!(fetched.user_id, bob);
    assert_eq!(fetched.name, "laptop");
    assert!(fetched.last_used_at.is_none());

    // Listed under the owner.
    let user_creds = passkey_repo::find_by_user(conn, bob).await.unwrap();
    assert_eq!(user_creds.len(), 1);

    // After successful auth the counter in credential_json advances
    // and last_used_at updates.
    let updated = passkey_repo::update_after_auth(conn, cred_id, r#"{"counter":1}"#.into(), 20)
        .await
        .expect("update after auth");
    assert_eq!(updated.credential_json, r#"{"counter":1}"#);
    assert_eq!(updated.last_used_at, Some(20));

    // Updating a non-existent credential surfaces NotFound.
    let missing = passkey_repo::update_after_auth(conn, Uuid::now_v7(), "{}".into(), 30).await;
    assert!(matches!(missing, Err(mmcp_db::DbError::NotFound)));

    // Delete, then verify it's gone.
    passkey_repo::delete(conn, cred_id).await.expect("delete");
    let gone = passkey_repo::find_by_id(conn, cred_id).await.unwrap();
    assert!(gone.is_none());

    // Deleting an already-absent id is idempotent (no error).
    passkey_repo::delete(conn, cred_id)
        .await
        .expect("delete idempotent");
}

#[tokio::test]
async fn user_lookup_by_id_and_require_and_update_profile() {
    let db = fresh_database().await;
    let conn = db.connection();

    let carol = Uuid::now_v7();
    user_repo::create(
        conn,
        user_repo::NewUser {
            id: carol,
            handle: "carol".into(),
            display_name: None,
            password_hash: None,
            email: Some("carol@example.com".into()),
            created_at: 1,
        },
    )
    .await
    .unwrap();

    // Look up by primary id: present path.
    let by_id = user_repo::find_by_id(conn, carol).await.unwrap().unwrap();
    assert_eq!(by_id.handle, "carol");

    // `require` returns the row for an existing id.
    let required = user_repo::require(conn, carol)
        .await
        .expect("require alice");
    assert_eq!(required.id, carol);

    // `require` surfaces NotFound for an unknown id rather than None.
    let err = user_repo::require(conn, Uuid::now_v7()).await.unwrap_err();
    assert!(matches!(err, mmcp_db::DbError::NotFound));

    // `update_profile` writes display_name and rotates the password
    // hash when supplied; the returned model reflects both changes.
    let updated =
        user_repo::update_profile(conn, carol, Some("Carol Q.".into()), Some("hash-v2".into()))
            .await
            .expect("update profile");
    assert_eq!(updated.display_name.as_deref(), Some("Carol Q."));
    assert_eq!(updated.password_hash.as_deref(), Some("hash-v2"));

    // Passing `None` for password_hash preserves the existing hash while still updating display_name:
    // the "edit profile but not password" path.
    let preserved = user_repo::update_profile(conn, carol, Some("Carol Third".into()), None)
        .await
        .expect("update profile without password");
    assert_eq!(preserved.display_name.as_deref(), Some("Carol Third"));
    assert_eq!(preserved.password_hash.as_deref(), Some("hash-v2"));

    // Updating a non-existent user surfaces NotFound via the
    // require() call inside update_profile.
    let missing = user_repo::update_profile(conn, Uuid::now_v7(), None, None).await;
    assert!(matches!(missing, Err(mmcp_db::DbError::NotFound)));
}
