//! Integration tests for the `/sync/*` control-plane routes.
//!
//! The baseline llvm-cov run flagged `mmcp-server::routes::sync` at
//! 5% line coverage — the happy paths of `GET /sync/manifest`,
//! `GET /sync/refs/{group}`, and `POST /sync/push` were effectively
//! unverified. These tests drive a real axum server with an in-
//! memory database and tempdir-backed repo root, hit each endpoint
//! via `reqwest`, and assert the shapes against the wire types
//! shared with the `mmcp-sync` client crate.

use std::net::SocketAddr;

use mmcp_core::id::GroupId;
use mmcp_core::manifest::GroupManifest;
use mmcp_core::memory::BumpIntent;
use mmcp_db::entities::group::OwnerKind;
use mmcp_db::repository::{group_repo, user_repo};
use mmcp_git::{CommitSpec, GitBackend};
use mmcp_sync::{ManifestResponse, PushRequest, PushResponse, RefsResponse};
use tempfile::TempDir;
use uuid::Uuid;

/// Spin up the real axum router on an ephemeral port so the tests
/// exercise handlers through the full HTTP stack, matching what a
/// real `mmcp-sync` client would send.
async fn start_server() -> (SocketAddr, mmcp_server::state::ServerState, TempDir) {
    let tmp = TempDir::new().expect("tempdir");
    let cfg = mmcp_server::config::ServerConfig {
        bind: "127.0.0.1:0".parse().unwrap(),
        database_url: "sqlite::memory:".to_string(),
        repo_root: tmp.path().to_path_buf(),
        token_key: [0u8; 32],
        oauth_providers: vec![],
        origin: "http://localhost:8787".to_string(),
    };
    let state = mmcp_server::state::ServerState::initialize(&cfg)
        .await
        .expect("server init");
    let app = mmcp_server::app::build_router(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("local addr");
    tokio::spawn(async move {
        axum::serve(listener, app.into_make_service())
            .await
            .unwrap();
    });
    (addr, state, tmp)
}

/// Insert a group row in the server DB and create its bare repo on
/// disk with the stock manifest commit. Returns the group UUID.
async fn seed_group(
    state: &mmcp_server::state::ServerState,
    slug: &str,
) -> Uuid {
    let group_id = GroupId::new();
    let uuid = *group_id.as_uuid();
    let owner = Uuid::now_v7();
    let manifest = GroupManifest::new_user_owned(group_id, slug, owner);
    state
        .git
        .create_group_repo(&manifest)
        .await
        .expect("create bare repo");
    group_repo::create(
        state.database.connection(),
        group_repo::NewGroup {
            id: uuid,
            slug: slug.into(),
            owner_kind: OwnerKind::User,
            owner_id: owner,
            display_name: None,
            created_at: jiff::Timestamp::now().as_second(),
        },
    )
    .await
    .expect("insert group row");
    uuid
}

#[tokio::test]
async fn sync_manifest_returns_empty_list_when_no_groups_exist() {
    let (addr, _state, _tmp) = start_server().await;
    let body: ManifestResponse = reqwest::get(format!("http://{addr}/sync/manifest"))
        .await
        .expect("GET manifest")
        .json()
        .await
        .expect("decode json");
    assert!(
        body.groups.is_empty(),
        "fresh server should advertise no groups; got {:?}",
        body.groups
    );
}

#[tokio::test]
async fn sync_manifest_lists_seeded_groups_with_head_commits() {
    let (addr, state, _tmp) = start_server().await;
    let group = seed_group(&state, "team-rust").await;

    let body: ManifestResponse = reqwest::get(format!("http://{addr}/sync/manifest"))
        .await
        .expect("GET manifest")
        .json()
        .await
        .expect("decode json");
    assert_eq!(body.groups.len(), 1);
    let entry = &body.groups[0];
    assert_eq!(entry.group_id, group);
    assert_eq!(entry.slug, "team-rust");
    // The manifest commit from `create_group_repo` gives a real
    // head commit — it must be a 40-char hex id, not the zero hash.
    assert_eq!(
        entry.head_commit.len(),
        40,
        "head_commit should be a real commit id"
    );
    assert_ne!(
        entry.head_commit,
        mmcp_core::conventions::ZERO_COMMIT,
        "head_commit should not be the zero hash for a group with a manifest"
    );
}

#[tokio::test]
async fn sync_refs_returns_main_tip_for_known_group() {
    let (addr, state, _tmp) = start_server().await;
    let group = seed_group(&state, "team-rust").await;

    let resp: RefsResponse = reqwest::get(format!("http://{addr}/sync/refs/{group}"))
        .await
        .expect("GET refs")
        .json()
        .await
        .expect("decode json");
    assert_eq!(resp.group_id, group);
    assert_eq!(resp.refs.len(), 1, "main should advertise one ref");
    assert_eq!(
        resp.refs[0].name,
        mmcp_core::conventions::MAIN_BRANCH_REF
    );
    assert_eq!(resp.refs[0].commit.len(), 40);
}

#[tokio::test]
async fn sync_refs_returns_404_for_unknown_group() {
    let (addr, _state, _tmp) = start_server().await;
    let unknown = Uuid::now_v7();
    let resp = reqwest::get(format!("http://{addr}/sync/refs/{unknown}"))
        .await
        .expect("GET refs");
    assert_eq!(resp.status(), 404);
}

#[tokio::test]
async fn sync_refs_returns_400_for_invalid_uuid() {
    let (addr, _state, _tmp) = start_server().await;
    let resp = reqwest::get(format!("http://{addr}/sync/refs/not-a-uuid"))
        .await
        .expect("GET refs");
    assert_eq!(resp.status(), 400);
}

#[tokio::test]
async fn sync_push_first_publish_assigns_0_1_0_and_records_tag() {
    let (addr, state, _tmp) = start_server().await;
    let group = seed_group(&state, "team-rust").await;

    // Use a real commit from the bare repo so the server's tag
    // creation can succeed and cover the tag-write branch too.
    let handle = mmcp_git::RepoHandle::new(
        group,
        state.group_repo_path(group).to_string_lossy().into_owned(),
    );
    let commit_id = state
        .git
        .write_commit(
            &handle,
            CommitSpec::mmcp_commit(
                "seed memory commit",
                vec![(
                    mmcp_core::conventions::legacy_memory_path("rules"),
                    Some(b"+++\nname = \"Rules\"\ndescription = \"A rule\"\nkind = \"rule\"\nmandatory = true\ntags = [\"test\"]\n+++\n\nBody.\n".to_vec()),
                )],
                "alice",
                "alice@example.com",
            ),
        )
        .await
        .expect("write commit");

    // post_push currently records the memory_version with
    // `author_id = memory.id`. That column is a FK onto `users`, so
    // without a matching user row the insert fails with a FK
    // constraint error. The production code has a TODO about wiring
    // this to the authenticated user id once auth lands; until then
    // we seed a user row whose UUID we reuse as the memory id so
    // the FK resolves and the happy path is actually exercised.
    let memory = Uuid::now_v7();
    user_repo::create(
        state.database.connection(),
        user_repo::NewUser {
            id: memory,
            handle: format!("placeholder-{}", memory.simple()),
            display_name: None,
            password_hash: None,
            email: None,
            created_at: jiff::Timestamp::now().as_millisecond(),
        },
    )
    .await
    .expect("seed placeholder user for FK");

    let req = PushRequest {
        group_id: group,
        memory_id: memory,
        commit: commit_id.clone(),
        bump: BumpIntent::Minor,
        message: Some("first publish".into()),
    };

    let raw = reqwest::Client::new()
        .post(format!("http://{addr}/sync/push"))
        .json(&req)
        .send()
        .await
        .expect("POST push");
    let status = raw.status();
    let text = raw.text().await.expect("read body");
    assert!(
        status.is_success(),
        "push returned {status}: {text}"
    );
    let resp: PushResponse = serde_json::from_str(&text).expect("decode push response");

    assert_eq!(resp.group_id, group);
    assert_eq!(resp.memory_id, memory);
    assert_eq!(resp.assigned_version, "0.1.0", "first publish is always 0.1.0");
    assert_eq!(resp.tag, "v0.1.0");
}

#[tokio::test]
async fn sync_push_returns_404_for_unknown_group() {
    let (addr, _state, _tmp) = start_server().await;
    let req = PushRequest {
        group_id: Uuid::now_v7(),
        memory_id: Uuid::now_v7(),
        commit: "0".repeat(40),
        bump: BumpIntent::Patch,
        message: None,
    };
    let resp = reqwest::Client::new()
        .post(format!("http://{addr}/sync/push"))
        .json(&req)
        .send()
        .await
        .expect("POST push");
    assert_eq!(resp.status(), 404);
}
