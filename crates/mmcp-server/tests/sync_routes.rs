#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Integration tests for the `/sync/*` control-plane routes.
//!
//! Covers the happy paths of `GET /sync/manifest`, `GET /sync/refs/{group}`, and `POST /sync/push`.
//! Tests drive a real axum server with an in-memory database and tempdir-backed repo root,
//! hit each endpoint via `reqwest`,
//! and assert the shapes against the wire types shared with the `mmcp-sync` client crate.

use std::net::SocketAddr;

use mmcp_auth::SessionClaims;
use mmcp_core::id::{GroupId, MemoryId, UserId};
use mmcp_core::manifest::GroupManifest;
use mmcp_core::memory::BumpIntent;
use mmcp_db::entities::group::OwnerKind;
use mmcp_db::entities::memory::MemoryKind;
use mmcp_db::entities::memory_version;
use mmcp_db::repository::{group_repo, memory_repo, user_repo};
use mmcp_git::{CommitSpec, GitBackend};
use mmcp_sync::{ManifestResponse, PushRequest, PushResponse, RefsResponse};
use tempfile::TempDir;
use uuid::Uuid;

/// Lifetime, in seconds, given to a token minted for the happy-path
/// tests below; long enough that no test run can plausibly cross it.
const TEST_TOKEN_LIFETIME_SECS: i64 = 3600;

/// Seconds by which the expired-token test backdates a claim's `exp`
/// (and `iat`, further still) so verification sees it as already
/// expired without needing to sleep past a real deadline.
const EXPIRED_TOKEN_BACKDATE_SECS: i64 = 100;

/// Shared push-token value for tests that must present the mmcp
/// issue #190 credential `POST /sync/push` now requires. The literal
/// value is arbitrary; only equality with the server's own configured
/// `push_token` matters.
const TEST_PUSH_TOKEN: &str = "sync-push-test-token";

/// HTTP header carrying [`TEST_PUSH_TOKEN`], matching
/// `mmcp_server::routes::defaults::PUSH_TOKEN_HEADER` (private to the
/// server crate, so pinned here as a literal like every other header
/// name in this file, e.g. `"www-authenticate"` below).
const PUSH_TOKEN_HEADER: &str = "x-mmcp-push-token";

mod common;

/// Spin up the real axum router on an ephemeral port so the tests
/// exercise handlers through the full HTTP stack, matching what a
/// real `mmcp-sync` client would send. No push token is configured;
/// use [`start_server_with_push_token`] for a test that needs
/// `POST /sync/push` to succeed.
async fn start_server() -> (SocketAddr, mmcp_server::state::ServerState, TempDir) {
    start_server_with_push_token(None).await
}

/// Same as [`start_server`], with the server's shared push-token
/// credential (`ServerConfig::push_token`) set to `push_token` when
/// `Some`.
async fn start_server_with_push_token(
    push_token: Option<&str>,
) -> (SocketAddr, mmcp_server::state::ServerState, TempDir) {
    let tmp = TempDir::new().expect("tempdir");
    let mut builder = common::TestServerConfigBuilder::new(tmp.path().to_path_buf());
    if let Some(push_token) = push_token {
        builder = builder.push_token(push_token);
    }
    let cfg = builder.build();
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
async fn seed_group(state: &mmcp_server::state::ServerState, slug: &str) -> Uuid {
    let group_id = GroupId::new();
    let uuid = *group_id.as_uuid();
    let owner = Uuid::now_v7();
    let manifest = GroupManifest::new_user_owned(group_id, slug, UserId::from_uuid(owner));
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

/// Insert a group row in the server DB WITHOUT creating its backing
/// bare git repo on disk, so any handler that reads the repo hits a
/// real `GitError::RepoNotFound` carrying the tempdir's absolute
/// server filesystem path.
async fn seed_group_without_repo(state: &mmcp_server::state::ServerState, slug: &str) -> Uuid {
    let group_id = GroupId::new();
    let uuid = *group_id.as_uuid();
    let owner = Uuid::now_v7();
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

/// Seed a real user row and mint a valid bearer token for it, the way
/// `routes::auth::login` does for a real client. Every `/sync/*`
/// handler now requires this header; returns the user id (so a test
/// can assert against it) alongside the token string.
async fn seed_authenticated_user(
    state: &mmcp_server::state::ServerState,
    handle: &str,
) -> (Uuid, String) {
    let user_id = Uuid::now_v7();
    user_repo::create(
        state.database.connection(),
        user_repo::NewUser {
            id: user_id,
            handle: handle.to_string(),
            display_name: None,
            password_hash: None,
            email: None,
            created_at: jiff::Timestamp::now().as_millisecond(),
        },
    )
    .await
    .expect("seed authenticated user");

    let now = jiff::Timestamp::now().as_second();
    let claims =
        SessionClaims::new_with_lifetime(user_id, Uuid::now_v7(), now, TEST_TOKEN_LIFETIME_SECS);
    let token = state.token_issuer.issue(&claims).expect("issue token");
    (user_id, token)
}

/// Seed a memory row directly under `owning_group`, with one already
/// recorded version, so a test can assert a rejected push left a
/// KNOWN prior state byte-identical, not just "still exists".
/// `author_id` on the seeded `memory_versions` row is a real user row
/// id (an FK, `RESTRICT`), same constraint the handler itself is
/// bound by. Returns the memory id.
async fn seed_memory_with_version(
    state: &mmcp_server::state::ServerState,
    owning_group: Uuid,
    author_id: Uuid,
    version: &str,
) -> Uuid {
    let memory_id = Uuid::now_v7();
    let now = jiff::Timestamp::now().as_millisecond();
    memory_repo::create(
        state.database.connection(),
        memory_repo::NewMemory {
            id: memory_id,
            group_id: owning_group,
            slug: memory_id.to_string(),
            kind: MemoryKind::Rule,
            mandatory: false,
            created_at: now,
            updated_at: now,
        },
    )
    .await
    .expect("insert memory row");

    let version_row = memory_version::Model {
        id: Uuid::now_v7(),
        memory_id,
        version: version.to_string(),
        commit: "0".repeat(40),
        author_id,
        published_at: now,
        summary: None,
    };
    memory_repo::record_version(state.database.connection(), version_row)
        .await
        .expect("record initial version");
    memory_repo::set_latest_version(
        state.database.connection(),
        memory_id,
        version.to_string(),
        now,
    )
    .await
    .expect("set initial latest_version");

    memory_id
}

/// `global-security-rules` forbids HTTP error responses from
/// carrying internal exception text. Seed a group row with no
/// backing bare repo so `GET /sync/refs/{group}` hits a real
/// `GitError::RepoNotFound(<tempdir path>)` and assert the 500 body
/// contains neither the server filesystem path nor the underlying
/// error's own message text, only the shared generic message.
#[tokio::test]
async fn sync_refs_500_body_omits_internal_error_detail() {
    let (addr, state, tmp) = start_server().await;
    let group = seed_group_without_repo(&state, "team-rust").await;
    let (_user_id, token) = seed_authenticated_user(&state, "alice").await;

    let resp = reqwest::Client::new()
        .get(format!("http://{addr}/sync/refs/{group}"))
        .bearer_auth(&token)
        .send()
        .await
        .expect("GET refs");
    assert_eq!(resp.status(), 500);
    let text = resp.text().await.expect("read body");

    let tmp_path = tmp.path().to_string_lossy().into_owned();
    assert!(
        !text.contains(&tmp_path),
        "500 body must not leak the server's repo root path, got: {text}"
    );
    assert!(
        !text.to_lowercase().contains("repository not found"),
        "500 body must not leak the underlying GitError text, got: {text}"
    );

    let body: serde_json::Value = serde_json::from_str(&text).expect("decode json");
    assert_eq!(
        body["error"]["message"],
        mmcp_server::routes::response::GENERIC_INTERNAL_ERROR_MESSAGE,
        "500 body must carry only the shared generic message, got: {text}"
    );
}

#[tokio::test]
async fn sync_manifest_returns_empty_list_when_no_groups_exist() {
    let (addr, state, _tmp) = start_server().await;
    let (_user_id, token) = seed_authenticated_user(&state, "alice").await;
    let body: ManifestResponse = reqwest::Client::new()
        .get(format!("http://{addr}/sync/manifest"))
        .bearer_auth(&token)
        .send()
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
    let (_user_id, token) = seed_authenticated_user(&state, "alice").await;

    let body: ManifestResponse = reqwest::Client::new()
        .get(format!("http://{addr}/sync/manifest"))
        .bearer_auth(&token)
        .send()
        .await
        .expect("GET manifest")
        .json()
        .await
        .expect("decode json");
    assert_eq!(body.groups.len(), 1);
    let entry = &body.groups[0];
    assert_eq!(entry.group_id, group);
    assert_eq!(entry.slug, "team-rust");
    // The manifest commit from `create_group_repo` gives a real head commit: a 40-char hex id, never the zero hash.
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

/// Falsification for the tip-resolution fix: `/sync/manifest` must
/// report the branch's true tip, not the last commit that happened
/// to touch `.mmcp.toml`. A second commit that edits a memory file
/// without touching the manifest advances `main` past the manifest
/// commit; the old `walk_history(".mmcp.toml").next()` lookup would
/// still report the manifest commit here, one commit behind the
/// real tip.
#[tokio::test]
async fn sync_manifest_reports_true_tip_past_the_last_manifest_touching_commit() {
    let (addr, state, _tmp) = start_server().await;
    let group = seed_group(&state, "team-rust").await;
    let (_user_id, token) = seed_authenticated_user(&state, "alice").await;

    let handle = mmcp_git::RepoHandle::new(
        group,
        state.group_repo_path(group).to_string_lossy().into_owned(),
    );
    let second_commit = state
        .git
        .write_commit(
            &handle,
            CommitSpec::mmcp_commit(
                "seed memory commit",
                vec![(
                    mmcp_core::conventions::memory_path("rules", MemoryId::new()),
                    Some(b"+++\nname = \"Rules\"\ndescription = \"A rule\"\nkind = \"rule\"\nmandatory = true\ntags = [\"test\"]\n+++\n\nBody.\n".to_vec()),
                )],
                "alice",
                "alice@example.com",
            ),
        )
        .await
        .expect("write second commit");

    let body: ManifestResponse = reqwest::Client::new()
        .get(format!("http://{addr}/sync/manifest"))
        .bearer_auth(&token)
        .send()
        .await
        .expect("GET manifest")
        .json()
        .await
        .expect("decode json");
    let entry = body
        .groups
        .iter()
        .find(|g| g.group_id == group)
        .expect("seeded group present in manifest");
    assert_eq!(
        entry.head_commit, second_commit,
        "manifest must report the true branch tip, not the last commit that touched .mmcp.toml"
    );
}

/// Falsification test for the bounded-concurrency rewrite of
/// `get_manifest`'s per-row loop: with more rows than
/// `MAX_CONCURRENT_MANIFEST_LOOKUPS`, every seeded group must still
/// appear exactly once with its own real head commit, never dropped,
/// duplicated, or cross-assigned to the wrong slug by the concurrent
/// scheduling.
#[tokio::test]
async fn sync_manifest_lists_every_seeded_group_under_concurrent_lookups() {
    let (addr, state, _tmp) = start_server().await;
    let (_user_id, token) = seed_authenticated_user(&state, "alice").await;
    let mut seeded = Vec::new();
    for i in 0..10 {
        let slug = format!("team-{i:02}");
        let group_id = seed_group(&state, &slug).await;
        seeded.push((group_id, slug));
    }

    let body: ManifestResponse = reqwest::Client::new()
        .get(format!("http://{addr}/sync/manifest"))
        .bearer_auth(&token)
        .send()
        .await
        .expect("GET manifest")
        .json()
        .await
        .expect("decode json");
    assert_eq!(body.groups.len(), seeded.len());

    for (group_id, slug) in &seeded {
        let entry = body
            .groups
            .iter()
            .find(|g| g.group_id == *group_id)
            .unwrap_or_else(|| panic!("group {group_id} missing from manifest response"));
        assert_eq!(&entry.slug, slug);
        assert_eq!(entry.head_commit.len(), 40, "real commit id for {slug}");
        assert_ne!(entry.head_commit, mmcp_core::conventions::ZERO_COMMIT);
    }
}

#[tokio::test]
async fn sync_refs_returns_main_tip_for_known_group() {
    let (addr, state, _tmp) = start_server().await;
    let group = seed_group(&state, "team-rust").await;
    let (_user_id, token) = seed_authenticated_user(&state, "alice").await;

    let resp: RefsResponse = reqwest::Client::new()
        .get(format!("http://{addr}/sync/refs/{group}"))
        .bearer_auth(&token)
        .send()
        .await
        .expect("GET refs")
        .json()
        .await
        .expect("decode json");
    assert_eq!(resp.group_id, group);
    assert_eq!(resp.refs.len(), 1, "main should advertise one ref");
    assert_eq!(resp.refs[0].name, mmcp_core::conventions::MAIN_BRANCH_REF);
    assert_eq!(resp.refs[0].commit.len(), 40);
}

#[tokio::test]
async fn sync_refs_returns_404_for_unknown_group() {
    let (addr, state, _tmp) = start_server().await;
    let (_user_id, token) = seed_authenticated_user(&state, "alice").await;
    let unknown = Uuid::now_v7();
    let resp = reqwest::Client::new()
        .get(format!("http://{addr}/sync/refs/{unknown}"))
        .bearer_auth(&token)
        .send()
        .await
        .expect("GET refs");
    assert_eq!(resp.status(), 404);
}

#[tokio::test]
async fn sync_refs_returns_400_for_invalid_uuid() {
    let (addr, state, _tmp) = start_server().await;
    let (_user_id, token) = seed_authenticated_user(&state, "alice").await;
    let resp = reqwest::Client::new()
        .get(format!("http://{addr}/sync/refs/not-a-uuid"))
        .bearer_auth(&token)
        .send()
        .await
        .expect("GET refs");
    assert_eq!(resp.status(), 400);
}

/// Also covers mmcp issue #190's positive path: `POST /sync/push`
/// now requires the shared push-token credential ALONGSIDE the
/// caller's per-user bearer, and this is the happy-path proof that
/// presenting both together still succeeds.
#[tokio::test]
async fn sync_push_first_publish_assigns_0_1_0_and_records_tag() {
    let (addr, state, _tmp) = start_server_with_push_token(Some(TEST_PUSH_TOKEN)).await;
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
                    mmcp_core::conventions::memory_path("rules", MemoryId::new()),
                    Some(b"+++\nname = \"Rules\"\ndescription = \"A rule\"\nkind = \"rule\"\nmandatory = true\ntags = [\"test\"]\n+++\n\nBody.\n".to_vec()),
                )],
                "alice",
                "alice@example.com",
            ),
        )
        .await
        .expect("write commit");

    // `author_id` on the inserted `memory_versions` row is a FK onto
    // `users` (RESTRICT), so the authenticated caller must be a real
    // seeded user row. `post_push` takes that id from the bearer
    // token, never from the memory being published.
    let (user_id, token) = seed_authenticated_user(&state, "alice").await;
    let memory = Uuid::now_v7();

    let req = PushRequest {
        group_id: group,
        memory_id: memory,
        commit: commit_id.clone(),
        bump: BumpIntent::Minor,
        message: Some("first publish".into()),
    };

    let raw = reqwest::Client::new()
        .post(format!("http://{addr}/sync/push"))
        .bearer_auth(&token)
        .header(PUSH_TOKEN_HEADER, TEST_PUSH_TOKEN)
        .json(&req)
        .send()
        .await
        .expect("POST push");
    let status = raw.status();
    let text = raw.text().await.expect("read body");
    assert!(status.is_success(), "push returned {status}: {text}");
    let resp: PushResponse = serde_json::from_str(&text).expect("decode push response");

    assert_eq!(resp.group_id, group);
    assert_eq!(resp.memory_id, memory);
    assert_eq!(
        resp.assigned_version, "0.1.0",
        "first publish is always 0.1.0"
    );
    assert_eq!(resp.tag, "v0.1.0");

    // The FK-authority claim itself: the inserted row's `author_id`
    // is the authenticated caller, read back from the database, not
    // inferred from the HTTP 2xx alone.
    let versions = memory_repo::list_versions(state.database.connection(), memory)
        .await
        .expect("list versions");
    assert_eq!(versions.len(), 1);
    assert_eq!(
        versions[0].author_id, user_id,
        "author_id must be the bearer-authenticated caller, not the memory's own id"
    );
}

/// The push token is presented so this test still isolates the
/// unknown-group 404 path: without it, an unknown group_id would be
/// rejected 403 by the push-token gate (mmcp issue #190) before the
/// group lookup ever runs, testing the wrong thing.
#[tokio::test]
async fn sync_push_returns_404_for_unknown_group() {
    let (addr, state, _tmp) = start_server_with_push_token(Some(TEST_PUSH_TOKEN)).await;
    let (_user_id, token) = seed_authenticated_user(&state, "alice").await;
    let req = PushRequest {
        group_id: Uuid::now_v7(),
        memory_id: Uuid::now_v7(),
        commit: "0".repeat(40),
        bump: BumpIntent::Patch,
        message: None,
    };
    let resp = reqwest::Client::new()
        .post(format!("http://{addr}/sync/push"))
        .bearer_auth(&token)
        .header(PUSH_TOKEN_HEADER, TEST_PUSH_TOKEN)
        .json(&req)
        .send()
        .await
        .expect("POST push");
    assert_eq!(resp.status(), 404);
}

/// Falsification for mmcp issue #190: a valid per-user bearer token
/// alone must not authorize `POST /sync/push`; the shared push-token
/// credential `git-receive-pack`'s `enforce_write` already requires
/// is now required here too, for the same write capability
/// (`memory_versions` row + git tag). Status matches
/// `enforce_write`'s own semantics for "token configured, none
/// presented": 401, not 403 (403 is reserved for "no token
/// configured at all").
#[tokio::test]
async fn sync_push_without_push_token_header_returns_401() {
    let (addr, state, _tmp) = start_server_with_push_token(Some(TEST_PUSH_TOKEN)).await;
    let group = seed_group(&state, "team-rust").await;
    let (_user_id, token) = seed_authenticated_user(&state, "alice").await;
    let req = PushRequest {
        group_id: group,
        memory_id: Uuid::now_v7(),
        commit: "0".repeat(40),
        bump: BumpIntent::Patch,
        message: None,
    };
    let resp = reqwest::Client::new()
        .post(format!("http://{addr}/sync/push"))
        .bearer_auth(&token)
        .json(&req)
        .send()
        .await
        .expect("POST push");
    assert_eq!(
        resp.status(),
        401,
        "a valid per-user bearer without the push token must not authorize the write"
    );
}

/// Falsification for mmcp issue #283: `memory_repo::find_by_id`
/// resolves `memory_id` independently of `group_id`, so `post_push`
/// must itself reject a push that names a real `memory_id` alongside
/// a `group_id` the memory does not belong to, BEFORE writing
/// anything. Confirms zero mutation of the victim's version ledger,
/// not just the rejected status code.
#[tokio::test]
async fn sync_push_rejects_a_memory_owned_by_a_different_group_without_mutating_it() {
    let (addr, state, _tmp) = start_server_with_push_token(Some(TEST_PUSH_TOKEN)).await;

    let victim_group = seed_group(&state, "victim-group").await;
    let attacker_group = seed_group(&state, "attacker-group").await;
    let (owner_id, _owner_token) = seed_authenticated_user(&state, "victim-owner").await;
    let memory_id = seed_memory_with_version(&state, victim_group, owner_id, "0.1.0").await;
    let (_attacker_id, attacker_token) = seed_authenticated_user(&state, "attacker").await;

    let req = PushRequest {
        group_id: attacker_group,
        memory_id,
        commit: "1".repeat(40),
        bump: BumpIntent::Major,
        message: Some("hostile takeover".into()),
    };

    let raw = reqwest::Client::new()
        .post(format!("http://{addr}/sync/push"))
        .bearer_auth(&attacker_token)
        .header(PUSH_TOKEN_HEADER, TEST_PUSH_TOKEN)
        .json(&req)
        .send()
        .await
        .expect("POST push");
    let status = raw.status();
    let text = raw.text().await.expect("read body");
    assert_eq!(
        status, 403,
        "a memory owned by a different group must be rejected, got {status}: {text}"
    );
    assert!(
        !text.contains(&victim_group.to_string()),
        "the rejection must not leak the memory's real owning group id, got: {text}"
    );

    // Zero-mutation: the victim's version ledger is byte-identical to
    // before the rejected push.
    let memory = memory_repo::find_by_id(state.database.connection(), memory_id)
        .await
        .expect("query memory")
        .expect("memory row still exists");
    assert_eq!(
        memory.latest_version.as_deref(),
        Some("0.1.0"),
        "latest_version must be untouched by the rejected cross-tenant push"
    );
    assert_eq!(
        memory.group_id, victim_group,
        "the memory's owning group must not change either"
    );
    let versions = memory_repo::list_versions(state.database.connection(), memory_id)
        .await
        .expect("list versions");
    assert_eq!(
        versions.len(),
        1,
        "no new memory_versions row must be written by the rejected push"
    );
}

/// Falsification: an unauthenticated `POST /sync/push` must be
/// rejected before it ever reaches the handler body, never a 2xx.
#[tokio::test]
async fn sync_push_without_bearer_token_returns_401() {
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
    assert_eq!(resp.status(), 401);
    assert_eq!(
        resp.headers()
            .get("www-authenticate")
            .map(|v| v.to_str().unwrap()),
        Some(r#"Bearer realm="mmcp""#),
        "401 must challenge with the Bearer scheme the extractor actually accepts"
    );
}

/// Falsification: an unauthenticated `GET /sync/manifest` must not
/// return every group in the server to an anonymous caller.
#[tokio::test]
async fn sync_manifest_without_bearer_token_returns_401() {
    let (addr, state, _tmp) = start_server().await;
    seed_group(&state, "team-rust").await;

    let resp = reqwest::get(format!("http://{addr}/sync/manifest"))
        .await
        .expect("GET manifest");
    assert_eq!(resp.status(), 401);
}

/// A lowercase `authorization: bearer <token>` scheme must be
/// rejected exactly like a missing header: `strip_prefix("Bearer ")`
/// is case-sensitive by design (same precedent as `git_http.rs`'s
/// `enforce_write`), so this pins the current fail-closed behavior
/// rather than changing it.
#[tokio::test]
async fn sync_manifest_with_lowercase_bearer_scheme_is_rejected_like_a_missing_header() {
    let (addr, state, _tmp) = start_server().await;
    let (_user_id, token) = seed_authenticated_user(&state, "alice").await;

    let resp = reqwest::Client::new()
        .get(format!("http://{addr}/sync/manifest"))
        .header("Authorization", format!("bearer {token}"))
        .send()
        .await
        .expect("GET manifest");
    assert_eq!(resp.status(), 401);
    assert_eq!(
        resp.headers()
            .get("www-authenticate")
            .map(|v| v.to_str().unwrap()),
        Some(r#"Bearer realm="mmcp""#),
        "a lowercase scheme must be treated as no bearer token at all, same challenge as a \
         missing header"
    );
}

/// Falsification: an unauthenticated `GET /sync/refs/{group_id}` must
/// not disclose a group's refs to an anonymous caller.
#[tokio::test]
async fn sync_refs_without_bearer_token_returns_401() {
    let (addr, state, _tmp) = start_server().await;
    let group = seed_group(&state, "team-rust").await;

    let resp = reqwest::get(format!("http://{addr}/sync/refs/{group}"))
        .await
        .expect("GET refs");
    assert_eq!(resp.status(), 401);
}

/// A structurally malformed bearer token (never issued by this
/// server's `TokenIssuer`) must fail verification cleanly: 401, never
/// a panic or a 500.
#[tokio::test]
async fn sync_manifest_with_malformed_token_returns_401_not_panic() {
    let (addr, _state, _tmp) = start_server().await;

    let resp = reqwest::Client::new()
        .get(format!("http://{addr}/sync/manifest"))
        .bearer_auth("not-a-real-paseto-token")
        .send()
        .await
        .expect("GET manifest");
    assert_eq!(resp.status(), 401);
}

/// A structurally valid token whose `exp` claim is already in the
/// past must be rejected: 401, never treated as authenticated.
#[tokio::test]
async fn sync_manifest_with_expired_token_returns_401() {
    let (addr, state, _tmp) = start_server().await;
    let (user_id, _valid_token) = seed_authenticated_user(&state, "alice").await;

    let now = jiff::Timestamp::now().as_second();
    let expired_claims = SessionClaims {
        sub: user_id,
        jti: Uuid::now_v7(),
        iat: now - (2 * EXPIRED_TOKEN_BACKDATE_SECS),
        exp: now - EXPIRED_TOKEN_BACKDATE_SECS,
    };
    let expired_token = state
        .token_issuer
        .issue(&expired_claims)
        .expect("issue expired token");

    let resp = reqwest::Client::new()
        .get(format!("http://{addr}/sync/manifest"))
        .bearer_auth(&expired_token)
        .send()
        .await
        .expect("GET manifest");
    assert_eq!(resp.status(), 401);
}
