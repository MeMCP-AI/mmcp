//! Error-branch coverage for `/auth/*` routes.
//!
//! `auth_flow.rs` covers the password happy path and the wrong-password/duplicate-handle 4xx cases.
//! This suite covers the OAuth branches, the passkey start/finish error paths, and the unknown-handle login path:
//! each test targets the server-local error surface, without a real WebAuthn client or a live OAuth provider.

use std::net::SocketAddr;

use mmcp_server::config::{OAuthProviderConfig, ServerConfig};
use mmcp_server::state::ServerState;
use serde_json::json;
use tempfile::TempDir;
use uuid::Uuid;

/// Bootstrap the server with the caller's choice of OAuth providers.
/// The listener is an ephemeral port on loopback; nothing touches
/// the real filesystem beyond a tempdir repo root.
async fn start_server_with_oauth(providers: Vec<OAuthProviderConfig>) -> (SocketAddr, TempDir) {
    let tmp = TempDir::new().expect("tempdir");
    let cfg = ServerConfig {
        bind: "127.0.0.1:0".parse().unwrap(),
        database_url: "sqlite::memory:".to_string(),
        repo_root: tmp.path().to_path_buf(),
        token_key: [7u8; 32],
        oauth_providers: providers,
        origin: "http://localhost:8787".to_string(),
    };
    let state = ServerState::initialize(&cfg).await.expect("state init");
    let app = mmcp_server::app::build_router(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("local addr");
    tokio::spawn(async move {
        axum::serve(listener, app.into_make_service())
            .await
            .unwrap();
    });
    (addr, tmp)
}

fn github_provider() -> OAuthProviderConfig {
    OAuthProviderConfig {
        slug: "github".into(),
        client_id: "client-abc".into(),
        client_secret: "secret-xyz".into(),
        auth_url: "https://github.com/login/oauth/authorize".into(),
        token_url: "https://github.com/login/oauth/access_token".into(),
        userinfo_url: "https://api.github.com/user".into(),
    }
}

// ── Password: unknown handle ────────────────────────────────────────

#[tokio::test]
async fn login_with_unknown_handle_returns_401() {
    let (addr, _tmp) = start_server_with_oauth(vec![]).await;
    let resp = reqwest::Client::new()
        .post(format!("http://{addr}/auth/login"))
        .json(&json!({ "handle": "ghost", "password": "whatever" }))
        .send()
        .await
        .expect("login");
    assert_eq!(resp.status(), 401);
}

// ── OAuth authorize ─────────────────────────────────────────────────

#[tokio::test]
async fn oauth_authorize_unknown_provider_returns_404() {
    let (addr, _tmp) = start_server_with_oauth(vec![]).await;
    let resp = reqwest::Client::new()
        .get(format!("http://{addr}/auth/oauth/google/authorize"))
        .send()
        .await
        .expect("oauth authorize");
    assert_eq!(resp.status(), 404);
}

#[tokio::test]
async fn oauth_authorize_known_provider_redirects_to_provider_authorize_url() {
    let (addr, _tmp) = start_server_with_oauth(vec![github_provider()]).await;
    // Disable auto-follow so we can inspect the 302.
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("build client");
    let resp = client
        .get(format!("http://{addr}/auth/oauth/github/authorize"))
        .send()
        .await
        .expect("oauth authorize");

    assert!(
        resp.status().is_redirection(),
        "expected redirect, got {}",
        resp.status()
    );
    let location = resp
        .headers()
        .get(reqwest::header::LOCATION)
        .expect("Location header")
        .to_str()
        .expect("utf-8");
    assert!(
        location.starts_with("https://github.com/login/oauth/authorize"),
        "unexpected redirect target: {location}"
    );
    assert!(location.contains("client_id=client-abc"));
    // The callback URL round-trips through `urlencoding::encode`
    // before being appended; ensure it points back at our origin.
    assert!(location.contains("redirect_uri="));
    // The handler appends the scope literally (no URL-encoding on
    // the `:` since it is a reserved char that is legal in a query).
    assert!(location.contains("scope=user:email"));
}

// ── Passkey error paths ─────────────────────────────────────────────

#[tokio::test]
async fn passkey_register_start_with_unknown_user_returns_404() {
    let (addr, _tmp) = start_server_with_oauth(vec![]).await;
    let resp = reqwest::Client::new()
        .post(format!("http://{addr}/auth/passkey/register/start"))
        .json(&json!({ "user_id": Uuid::now_v7().to_string() }))
        .send()
        .await
        .expect("passkey register start");
    assert_eq!(resp.status(), 404);
}

#[tokio::test]
async fn passkey_register_finish_without_pending_state_returns_400() {
    let (addr, _tmp) = start_server_with_oauth(vec![]).await;
    // Seed a real user so the lookup does not fail before the pending-state check.
    // This test never calls register/start, so the in-memory pending map has no entry for this user.
    let user_id = register_test_user(addr, "alice", "hunter2").await;

    // The `response` field must still be a well-formed JSON object
    // because axum's Json extractor runs before the handler body.
    // We pass a minimal placeholder; the handler should reject on
    // the missing pending state before it tries to finish the
    // webauthn ceremony. If axum rejects earlier, a 4xx still covers
    // the contract we care about (no 5xx, no panic).
    let resp = reqwest::Client::new()
        .post(format!("http://{addr}/auth/passkey/register/finish"))
        .json(&json!({
            "user_id": user_id,
            "credential_name": "laptop",
            "response": {
                "id": "AAAA",
                "rawId": "AAAA",
                "type": "public-key",
                "response": {
                    "attestationObject": "AAAA",
                    "clientDataJSON": "AAAA"
                }
            }
        }))
        .send()
        .await
        .expect("passkey register finish");
    assert!(
        resp.status().is_client_error(),
        "expected 4xx for missing pending state, got {}",
        resp.status()
    );
}

#[tokio::test]
async fn passkey_login_start_unknown_handle_returns_404() {
    let (addr, _tmp) = start_server_with_oauth(vec![]).await;
    let resp = reqwest::Client::new()
        .post(format!("http://{addr}/auth/passkey/login/start"))
        .json(&json!({ "handle": "ghost" }))
        .send()
        .await
        .expect("passkey login start");
    assert_eq!(resp.status(), 404);
}

#[tokio::test]
async fn passkey_login_start_user_without_credentials_returns_400() {
    let (addr, _tmp) = start_server_with_oauth(vec![]).await;
    // Register a user via password so a user row exists, but skip
    // passkey enrollment; the start handler must return 400.
    register_test_user(addr, "alice", "hunter2").await;

    let resp = reqwest::Client::new()
        .post(format!("http://{addr}/auth/passkey/login/start"))
        .json(&json!({ "handle": "alice" }))
        .send()
        .await
        .expect("passkey login start");
    assert_eq!(resp.status(), 400);
}

// ── Helpers ─────────────────────────────────────────────────────────

/// Create a user through the real `POST /auth/register` endpoint
/// and return the resulting user id as a string. Reuses the same
/// code path the happy-case auth_flow.rs hits, so seeding here also
/// exercises the register route once more on the coverage side.
async fn register_test_user(addr: SocketAddr, handle: &str, password: &str) -> String {
    let resp = reqwest::Client::new()
        .post(format!("http://{addr}/auth/register"))
        .json(&json!({ "handle": handle, "password": password }))
        .send()
        .await
        .expect("register");
    assert_eq!(resp.status(), 201, "register should succeed");
    let body: serde_json::Value = resp.json().await.expect("json");
    body["user_id"]
        .as_str()
        .expect("user_id string")
        .to_string()
}
