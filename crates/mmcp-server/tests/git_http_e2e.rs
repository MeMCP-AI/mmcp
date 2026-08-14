//! End-to-end smart-HTTP test against stock `git`.
//!
//! Spins up the real axum server on an ephemeral port, seeds one
//! group repo on disk with a committed memory, then drives a stock
//! `git clone` against the `/git/<uuid>.git` route. Passing confirms
//! the three pack endpoints (`info/refs`, `upload-pack`, plus the
//! pkt-line framing and content types) behave the way any vanilla
//! git binary expects, not only mmcp's own client.

use std::net::SocketAddr;

use mmcp_core::id::{GroupId, UserId};
use mmcp_core::manifest::GroupManifest;
use mmcp_db::entities::group::OwnerKind;
use mmcp_db::repository::group_repo;
use mmcp_git::{CommitSpec, GitBackend};
use tempfile::TempDir;
use uuid::Uuid;

/// Start the server on an ephemeral port, returning its address and
/// the `ServerState` handle so the test can seed groups into it.
async fn start_server(
    repo_root: &std::path::Path,
) -> (SocketAddr, mmcp_server::state::ServerState) {
    let cfg = mmcp_server::config::ServerConfig {
        bind: "127.0.0.1:0".parse().unwrap(),
        database_url: "sqlite::memory:".to_string(),
        repo_root: repo_root.to_path_buf(),
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

    let clone_tmp = TempDir::new().expect("clone tmp");
    let clone_dst = clone_tmp.path().join("clone");
    let url = format!("http://{addr}/git/{group_id}.git");

    let output = tokio::process::Command::new(&git_bin)
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
