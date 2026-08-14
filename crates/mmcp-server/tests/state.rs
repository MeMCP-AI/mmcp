//! Integration tests for `ServerState` helpers that are easier to
//! verify against a real initialized state than through a mock.

use std::sync::Arc;

use tempfile::TempDir;
use uuid::Uuid;

use mmcp_server::{config::ServerConfig, state::ServerState};

/// Spin up a full `ServerState` backed by an in-memory SQLite and a
/// tempdir-hosted repo root. Mirrors the bootstrap used by
/// `tests/health.rs` and `tests/auth_flow.rs`.
async fn bootstrap_state() -> (ServerState, TempDir) {
    let tmp = TempDir::new().expect("tempdir");
    let cfg = ServerConfig {
        bind: "127.0.0.1:0".parse().unwrap(),
        database_url: "sqlite::memory:".to_string(),
        repo_root: tmp.path().to_path_buf(),
        token_key: [0u8; 32],
        oauth_providers: vec![],
        origin: "http://localhost:8787".to_string(),
        push_token: None,
        min_password_length: mmcp_auth::MIN_PASSWORD_LENGTH,
        max_password_length: mmcp_auth::MAX_PASSWORD_LENGTH,
    };
    let state = ServerState::initialize(&cfg).await.expect("state init");
    (state, tmp)
}

#[tokio::test]
async fn repo_write_lock_returns_the_same_handle_for_the_same_group_id() {
    let (state, _tmp) = bootstrap_state().await;
    let group = Uuid::now_v7();

    let first = state.repo_write_lock(group);
    let second = state.repo_write_lock(group);

    // Same group → same underlying mutex. Two Arc clones that point at
    // one allocation (Arc::ptr_eq) prove the lazy-init path cached the
    // mutex on first call and reused it on second.
    assert!(
        Arc::ptr_eq(&first, &second),
        "repo_write_lock must hand out the same mutex for a given group"
    );
}

#[tokio::test]
async fn repo_write_lock_returns_distinct_handles_for_different_groups() {
    let (state, _tmp) = bootstrap_state().await;
    let a = state.repo_write_lock(Uuid::now_v7());
    let b = state.repo_write_lock(Uuid::now_v7());

    assert!(
        !Arc::ptr_eq(&a, &b),
        "distinct groups must receive distinct mutexes so writes to one do not serialize against the other"
    );
}

#[tokio::test]
async fn repo_write_lock_is_actually_held_when_guarded() {
    let (state, _tmp) = bootstrap_state().await;
    let group = Uuid::now_v7();
    let lock = state.repo_write_lock(group);

    // Holding the guard blocks a second `try_lock` on the same mutex,
    // which is exactly what serializes concurrent receive-pack calls.
    let _held = lock.clone().lock_owned().await;
    assert!(lock.try_lock().is_err(), "the mutex must be held");
}

#[tokio::test]
async fn group_repo_path_follows_uuid_dot_git_convention() {
    let (state, _tmp) = bootstrap_state().await;
    let group = Uuid::now_v7();
    let path = state.group_repo_path(group);
    let last = path
        .file_name()
        .expect("last path component")
        .to_string_lossy()
        .into_owned();
    assert!(last.ends_with(".git"));
    assert!(last.starts_with(&group.to_string()));
}
