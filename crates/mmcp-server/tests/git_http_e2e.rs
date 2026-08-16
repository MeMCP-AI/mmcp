#![allow(clippy::unwrap_used, clippy::expect_used)]
//! End-to-end smart-HTTP test against stock `git`.
//!
//! Spins up the real axum server on an ephemeral port, seeds one
//! group repo on disk with a committed memory, then drives a stock
//! `git clone` against the `/git/<uuid>.git` route. Passing confirms
//! the three pack endpoints (`info/refs`, `upload-pack`, plus the
//! pkt-line framing and content types) behave the way any vanilla
//! git binary expects, not only mmcp's own client.

use std::net::SocketAddr;

use mmcp_auth::SessionClaims;
use mmcp_core::id::{GroupId, UserId};
use mmcp_core::manifest::GroupManifest;
use mmcp_db::entities::group::OwnerKind;
use mmcp_db::repository::{group_repo, user_repo};
use mmcp_git::{CommitSpec, GitBackend};
use tempfile::TempDir;
use uuid::Uuid;

mod common;

/// Lifetime, in seconds, given to a token minted for these tests;
/// long enough that no test run can plausibly cross it.
const TEST_TOKEN_LIFETIME_SECS: i64 = 3600;

/// Build a stock `git` invocation with every interactive-credential
/// fallback suppressed: the terminal prompt, `GIT_ASKPASS`, and any
/// configured `credential.helper` (on Windows, typically Git
/// Credential Manager, which pops a real desktop dialog). Several
/// tests below intentionally drive a real `git` binary against an
/// endpoint that rejects the request (a missing or invalid bearer
/// token); without this, stock git's default reaction to that 401 is
/// to fall back to the ambient interactive credential machinery,
/// which blocks on (or pops) a real prompt with no human present to
/// answer it in an automated test/CI run.
///
/// Unconditional suppression is safe here specifically because
/// nothing in a test process is ever a legitimate interactive CLI
/// session; contrast `mmcp_git::native::repo_ops`'s own
/// `apply_credentials`, which suppresses these same three things only
/// for its `BearerHttp`/`SshCommand` arms and deliberately leaves
/// `Credentials::None` untouched so production interactive use still
/// works.
fn suppressed_git_command(git_bin: &std::ffi::OsStr) -> tokio::process::Command {
    let mut cmd = tokio::process::Command::new(git_bin);
    cmd.arg("-c").arg("credential.helper=");
    cmd.env("GIT_TERMINAL_PROMPT", "0");
    cmd.env("GIT_ASKPASS", "");
    cmd
}

/// Seed a real user row and mint a valid bearer token for it, the way
/// `routes::auth::login` does for a real client. Every `git-upload-pack`
/// read (both the `info/refs` advertisement and the pack transfer
/// itself) now requires this header, same as the `/sync/*` control
/// plane.
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

/// Start the server on an ephemeral port, returning its address and
/// the `ServerState` handle so the test can seed groups into it.
async fn start_server(
    repo_root: &std::path::Path,
) -> (SocketAddr, mmcp_server::state::ServerState) {
    let cfg = common::TestServerConfigBuilder::new(repo_root.to_path_buf()).build();
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
    (addr, state)
}

/// Insert a group row in the server database and create its bare
/// repository on disk with one committed memory file. Returns the
/// group UUID so the test can target it via the git-HTTP route.
async fn seed_group_with_memory(
    state: &mmcp_server::state::ServerState,
    slug: &str,
    memory_path: &str,
    memory_body: &str,
) -> Uuid {
    let group_id = GroupId::new();
    let uuid = *group_id.as_uuid();
    let owner_id = Uuid::now_v7();

    // Create the bare repo on disk with the usual manifest commit.
    let manifest = GroupManifest::new_user_owned(group_id, slug, UserId::from_uuid(owner_id));
    let handle = state
        .git
        .create_group_repo(&manifest)
        .await
        .expect("create bare repo");

    // Commit one memory so the clone returns something meaningful.
    state
        .git
        .write_commit(
            &handle,
            CommitSpec {
                branch: "main".into(),
                author_name: "alice".into(),
                author_email: "alice@example.com".into(),
                message: format!("add {memory_path}"),
                files: vec![(memory_path.into(), Some(memory_body.as_bytes().to_vec()))],
            },
        )
        .await
        .expect("write commit");

    // Insert a matching row so `ensure_group` lets the route serve it.
    group_repo::create(
        state.database.connection(),
        group_repo::NewGroup {
            id: uuid,
            slug: slug.into(),
            owner_kind: OwnerKind::User,
            owner_id,
            display_name: None,
            created_at: jiff::Timestamp::now().as_second(),
        },
    )
    .await
    .expect("insert group row");

    uuid
}

#[tokio::test]
async fn stock_git_clones_from_smart_http_route() {
    // Resolve `git` via the same env shim the native backend uses,
    // so tests honour `MMCP_GIT_BIN` overrides in constrained CI
    // images the same way production does.
    let git_bin = std::env::var_os("MMCP_GIT_BIN").unwrap_or_else(|| "git".into());

    let repo_tmp = TempDir::new().expect("repo tmp");
    let (addr, state) = start_server(repo_tmp.path()).await;
    let group_id = seed_group_with_memory(
        &state,
        "team-rust",
        "memories/hello.md",
        "content from server",
    )
    .await;
    let (_user_id, token) = seed_authenticated_user(&state, "alice").await;

    let clone_tmp = TempDir::new().expect("clone tmp");
    let clone_dst = clone_tmp.path().join("clone");
    let url = format!("http://{addr}/git/{group_id}.git");

    let output = suppressed_git_command(&git_bin)
        .arg("-c")
        .arg(format!("http.extraHeader=Authorization: Bearer {token}"))
        .arg("clone")
        .arg(&url)
        .arg(&clone_dst)
        .output()
        .await
        .expect("spawn git clone");
    assert!(
        output.status.success(),
        "git clone exited non-zero. stdout={}, stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    // The memory should be in the clone at the expected path.
    let fetched =
        std::fs::read_to_string(clone_dst.join("memories/hello.md")).expect("read cloned file");
    assert_eq!(fetched, "content from server");

    // And the manifest committed by `create_group_repo` should also
    // be there, proving the whole tree round-tripped, not just the
    // latest commit.
    let manifest =
        std::fs::read_to_string(clone_dst.join(".mmcp.toml")).expect("read cloned manifest");
    assert!(
        manifest.contains("team-rust"),
        "manifest missing slug: {manifest}"
    );
}

/// Falsification for issue #241 (`critical-git-upload-pack-and-info-refs-serve-full-repo-content`):
/// a stock `git clone`, driven exactly like the happy-path test above
/// but WITHOUT the bearer header, must fail end to end against the
/// real `git` binary rather than silently succeeding in cloning the
/// full repo content. Before the fix, `info_refs`'s `git-upload-pack`
/// branch and the whole `upload_pack` handler had no guard at all, so
/// this exact clone would succeed with exit code 0.
#[tokio::test]
async fn stock_git_clone_without_bearer_token_fails() {
    let git_bin = std::env::var_os("MMCP_GIT_BIN").unwrap_or_else(|| "git".into());

    let repo_tmp = TempDir::new().expect("repo tmp");
    let (addr, state) = start_server(repo_tmp.path()).await;
    let group_id = seed_group_with_memory(
        &state,
        "team-rust",
        "memories/hello.md",
        "content from server",
    )
    .await;

    let clone_tmp = TempDir::new().expect("clone tmp");
    let clone_dst = clone_tmp.path().join("clone");
    let url = format!("http://{addr}/git/{group_id}.git");

    let output = suppressed_git_command(&git_bin)
        .arg("clone")
        .arg(&url)
        .arg(&clone_dst)
        .output()
        .await
        .expect("spawn git clone");
    assert!(
        !output.status.success(),
        "an unauthenticated clone must fail, not silently succeed; stdout={}, stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    assert!(
        !clone_dst.join("memories/hello.md").exists(),
        "no group content must reach disk from a rejected clone"
    );
}

/// Falsification for issue #241, isolating the `info_refs` branch
/// directly (below stock `git`'s own retry/credential-helper layer):
/// an unauthenticated `GET .../info/refs?service=git-upload-pack`
/// must return 401 with the same Bearer challenge the `/sync/*`
/// control plane advertises, never the ref advertisement body.
#[tokio::test]
async fn info_refs_git_upload_pack_without_bearer_token_returns_401() {
    let repo_tmp = TempDir::new().expect("repo tmp");
    let (addr, state) = start_server(repo_tmp.path()).await;
    let group_id = seed_group_with_memory(
        &state,
        "team-rust",
        "memories/hello.md",
        "content from server",
    )
    .await;

    let resp = reqwest::get(format!(
        "http://{addr}/git/{group_id}.git/info/refs?service=git-upload-pack"
    ))
    .await
    .expect("GET info/refs");
    assert_eq!(resp.status(), 401);
    assert_eq!(
        resp.headers()
            .get("www-authenticate")
            .map(|v| v.to_str().unwrap()),
        Some(r#"Bearer realm="mmcp""#),
        "401 must challenge with the Bearer scheme the extractor actually accepts"
    );
}

/// Falsification for issue #241, isolating the `upload_pack` handler
/// directly: an unauthenticated `POST .../git-upload-pack` must
/// return 401, never dispatch into the gix pack-serve state machine.
#[tokio::test]
async fn upload_pack_post_without_bearer_token_returns_401() {
    let repo_tmp = TempDir::new().expect("repo tmp");
    let (addr, state) = start_server(repo_tmp.path()).await;
    let group_id = seed_group_with_memory(
        &state,
        "team-rust",
        "memories/hello.md",
        "content from server",
    )
    .await;

    let resp = reqwest::Client::new()
        .post(format!("http://{addr}/git/{group_id}.git/git-upload-pack"))
        .body(Vec::<u8>::new())
        .send()
        .await
        .expect("POST git-upload-pack");
    assert_eq!(resp.status(), 401);
}

/// The `git-receive-pack` advertisement branch must keep using the
/// shared-secret push-token mechanism (`enforce_write`), not the new
/// per-user bearer gate: this pins that the two mechanisms stay
/// distinct rather than the fix accidentally collapsing them.
#[tokio::test]
async fn info_refs_git_receive_pack_without_push_token_returns_403_not_401() {
    let repo_tmp = TempDir::new().expect("repo tmp");
    let (addr, state) = start_server(repo_tmp.path()).await;
    let group_id = seed_group_with_memory(
        &state,
        "team-rust",
        "memories/hello.md",
        "content from server",
    )
    .await;

    let resp = reqwest::get(format!(
        "http://{addr}/git/{group_id}.git/info/refs?service=git-receive-pack"
    ))
    .await
    .expect("GET info/refs");
    // `enforce_write` returns 403 (push disabled) when no push token
    // is configured, distinct from the 401 a bearer-auth rejection
    // would return; this test's own default `TestServerConfigBuilder`
    // config carries no push token.
    assert_eq!(resp.status(), 403);
}

/// Router-wide auth-posture regression test (the explicit ask of
/// issue #241, `critical-git-upload-pack-and-info-refs-serve-full-repo-content`):
/// every CONTENT-plane route `crate::routes::git_http::router()` (mirrored
/// here from that module's own doc comment listing the four routes)
/// must reject an unauthenticated caller. A future change that adds a
/// new git-HTTP route, or a new service branch on `info_refs`, and
/// forgets to gate it must fail this test by falling through the
/// catch-all "every entry rejected" assertion below rather than
/// silently shipping an open content-plane sibling next to a gated
/// control-plane route, exactly the asymmetry #241 found.
///
/// `git-receive-pack` is deliberately excluded from this table: it is
/// gated by the separate shared-secret push-token mechanism
/// (`enforce_write`), covered by its own dedicated test above and by
/// `crate::routes::git_http`'s in-crate `enforce_write` unit tests,
/// not by `AuthenticatedUser`.
#[tokio::test]
async fn every_upload_pack_route_rejects_an_unauthenticated_caller_router_wide() {
    let repo_tmp = TempDir::new().expect("repo tmp");
    let (addr, state) = start_server(repo_tmp.path()).await;
    let group_id = seed_group_with_memory(
        &state,
        "team-rust",
        "memories/hello.md",
        "content from server",
    )
    .await;

    let client = reqwest::Client::new();
    let routes: Vec<(&str, reqwest::Method, String)> = vec![
        (
            "GET /git/{group}.git/info/refs?service=git-upload-pack",
            reqwest::Method::GET,
            format!("http://{addr}/git/{group_id}.git/info/refs?service=git-upload-pack"),
        ),
        (
            "POST /git/{group}.git/git-upload-pack",
            reqwest::Method::POST,
            format!("http://{addr}/git/{group_id}.git/git-upload-pack"),
        ),
    ];

    for (label, method, url) in routes {
        let resp = client
            .request(method, &url)
            .body(Vec::<u8>::new())
            .send()
            .await
            .unwrap_or_else(|e| panic!("request to {label} failed to even send: {e}"));
        assert_eq!(
            resp.status(),
            401,
            "{label} must reject an unauthenticated caller with 401, got {}",
            resp.status()
        );
    }
}
