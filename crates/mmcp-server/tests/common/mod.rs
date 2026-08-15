//! Shared `ServerConfig` construction helper for mmcp-server's
//! integration tests.
//!
//! Every file under `tests/` compiles as its own independent crate,
//! so this module is pulled in via `mod common;` (never a peer
//! `use`) in each one that needs it. Centralizes the 10-field
//! `ServerConfig` literal that used to be hand-duplicated across
//! `auth_extras.rs`, `auth_flow.rs`, `git_http_e2e.rs`, `health.rs`,
//! `mcp_routes.rs`, `state.rs`, and `sync_routes.rs`.

use std::path::PathBuf;

use mmcp_server::config::ServerConfig;

/// Build the `ServerConfig` common to every integration test suite:
/// an ephemeral loopback bind, an in-memory SQLite database, no
/// OAuth providers, no push token, and the compiled-in
/// `mmcp_auth` password-length and handle-length defaults.
/// `repo_root` is the one field every caller must genuinely supply
/// (each test uses its own tempdir); `token_key` and
/// `oauth_providers` are the two fields that vary per test file, and
/// callers override them with struct-update syntax, e.g.
/// `ServerConfig { token_key: [7u8; 32],
/// ..common::test_server_config(repo_root) }`.
pub fn test_server_config(repo_root: PathBuf) -> ServerConfig {
    ServerConfig {
        bind: "127.0.0.1:0".parse().unwrap(),
        database_url: "sqlite::memory:".to_string(),
        repo_root,
        token_key: [0u8; 32],
        oauth_providers: vec![],
        origin: "http://localhost:8787".to_string(),
        push_token: None,
        min_password_length: mmcp_auth::MIN_PASSWORD_LENGTH,
        max_password_length: mmcp_auth::MAX_PASSWORD_LENGTH,
        max_handle_length: mmcp_auth::MAX_HANDLE_LENGTH,
    }
}
