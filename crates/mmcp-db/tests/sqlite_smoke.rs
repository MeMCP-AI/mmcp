//! End-to-end smoke test against an in-memory SQLite database.

use mmcp_db::entities::group::OwnerKind;
use mmcp_db::entities::memory::MemoryKind;
use mmcp_db::entities::membership::{GroupRole, PrincipalKind};
use mmcp_db::entities::{memory_read, memory_version, membership};
use mmcp_db::repository::{group_repo, memory_repo, org_repo, session_repo, user_repo};
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

    assert!(!session_repo::has_read(conn, "sess-1", memory_id)
        .await
        .unwrap());

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

    assert!(session_repo::has_read(conn, "sess-1", memory_id)
        .await
        .unwrap());

    session_repo::mark_post_compaction(conn, "sess-1", Some("sig".into()), 3)
        .await
        .unwrap();
    let refreshed = session_repo::find(conn, "sess-1").await.unwrap().unwrap();
    assert!(refreshed.post_compaction);

    session_repo::clear_post_compaction(conn, "sess-1").await.unwrap();
    let refreshed = session_repo::find(conn, "sess-1").await.unwrap().unwrap();
    assert!(!refreshed.post_compaction);
}
