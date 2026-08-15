//! Integration test: register + login flow.

use std::net::SocketAddr;

mod common;

async fn start_server() -> SocketAddr {
    start_server_with_config(
        common::TestServerConfigBuilder::new(tempfile::tempdir().unwrap().keep())
            .token_key([42u8; 32])
            .build(),
    )
    .await
}

/// Shared bootstrap behind [`start_server`] and every test that needs
/// a non-default `ServerConfig` (e.g. the `max_handle_length`
/// cascade falsification test below): builds the state and router
/// from a caller-supplied config, binds an ephemeral loopback port,
/// and spawns the server task.
async fn start_server_with_config(cfg: mmcp_server::config::ServerConfig) -> SocketAddr {
    let state = mmcp_server::state::ServerState::initialize(&cfg)
        .await
        .expect("server init");
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
    addr
}

#[tokio::test]
async fn register_then_login_succeeds() {
    let addr = start_server().await;
    let client = reqwest::Client::new();

    // Register.
    let resp = client
        .post(format!("http://{addr}/auth/register"))
        .json(&serde_json::json!({
            "handle": "alice",
            "password": "hunter22"
        }))
        .send()
        .await
        .expect("register");
    assert_eq!(resp.status(), 201, "register should return 201");
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(body["user_id"].is_string());

    // Login.
    let resp = client
        .post(format!("http://{addr}/auth/login"))
        .json(&serde_json::json!({
            "handle": "alice",
            "password": "hunter22"
        }))
        .send()
        .await
        .expect("login");
    assert_eq!(resp.status(), 200, "login should return 200");
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(body["token"].is_string());
    assert!(body["user_id"].is_string());
    assert!(body["expires_at"].is_number());
}

#[tokio::test]
async fn login_with_wrong_password_returns_401() {
    let addr = start_server().await;
    let client = reqwest::Client::new();

    // Register.
    client
        .post(format!("http://{addr}/auth/register"))
        .json(&serde_json::json!({
            "handle": "bob",
            "password": "correctpw"
        }))
        .send()
        .await
        .expect("register");

    // Login with wrong password.
    let resp = client
        .post(format!("http://{addr}/auth/login"))
        .json(&serde_json::json!({
            "handle": "bob",
            "password": "wrong"
        }))
        .send()
        .await
        .expect("login");
    assert_eq!(resp.status(), 401);
}

#[tokio::test]
async fn duplicate_register_returns_409() {
    let addr = start_server().await;
    let client = reqwest::Client::new();

    let body = serde_json::json!({
        "handle": "charlie",
        "password": "password"
    });

    let resp = client
        .post(format!("http://{addr}/auth/register"))
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 201);

    let resp = client
        .post(format!("http://{addr}/auth/register"))
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 409);
}

#[tokio::test]
async fn register_with_empty_password_returns_400() {
    let addr = start_server().await;
    let resp = reqwest::Client::new()
        .post(format!("http://{addr}/auth/register"))
        .json(&serde_json::json!({
            "handle": "dave",
            "password": ""
        }))
        .send()
        .await
        .expect("register");
    assert_eq!(resp.status(), 400);
}

#[tokio::test]
async fn register_with_whitespace_only_password_at_min_length_returns_201() {
    // 8 spaces meets the server's default `min_password_length` (8):
    // whitespace content is subject only to the length bound, like
    // any other password.
    let addr = start_server().await;
    let resp = reqwest::Client::new()
        .post(format!("http://{addr}/auth/register"))
        .json(&serde_json::json!({
            "handle": "erin",
            "password": "        "
        }))
        .send()
        .await
        .expect("register");
    assert_eq!(resp.status(), 201);
}

#[tokio::test]
async fn register_with_too_short_password_returns_400() {
    let addr = start_server().await;
    let resp = reqwest::Client::new()
        .post(format!("http://{addr}/auth/register"))
        .json(&serde_json::json!({
            "handle": "frank",
            "password": "short1"
        }))
        .send()
        .await
        .expect("register");
    assert_eq!(resp.status(), 400);
}

#[tokio::test]
async fn register_with_over_length_password_returns_400() {
    let addr = start_server().await;
    let resp = reqwest::Client::new()
        .post(format!("http://{addr}/auth/register"))
        .json(&serde_json::json!({
            "handle": "grace",
            "password": "a".repeat(mmcp_auth::MAX_PASSWORD_LENGTH + 1)
        }))
        .send()
        .await
        .expect("register");
    assert_eq!(resp.status(), 400);
}

#[tokio::test]
async fn register_with_empty_handle_returns_400() {
    let addr = start_server().await;
    let resp = reqwest::Client::new()
        .post(format!("http://{addr}/auth/register"))
        .json(&serde_json::json!({
            "handle": "   ",
            "password": "validpassword"
        }))
        .send()
        .await
        .expect("register");
    assert_eq!(resp.status(), 400);
}

#[tokio::test]
async fn register_with_over_length_handle_returns_400() {
    let addr = start_server().await;
    let resp = reqwest::Client::new()
        .post(format!("http://{addr}/auth/register"))
        .json(&serde_json::json!({
            "handle": "h".repeat(mmcp_server::routes::auth::MAX_HANDLE_LENGTH + 1),
            "password": "validpassword"
        }))
        .send()
        .await
        .expect("register");
    assert_eq!(resp.status(), 400);
}

/// `/auth/register` enforces the config-resolved bound, not the compiled-in default.
/// A handle under the 64-byte default but over a smaller configured bound must still be rejected.
#[tokio::test]
async fn overriding_max_handle_length_smaller_than_default_rejects_a_handle_the_default_would_accept()
 {
    const NARROWED_MAX_HANDLE_LENGTH: usize = 8;
    let handle = "h".repeat(NARROWED_MAX_HANDLE_LENGTH + 2);
    assert!(
        handle.len() < mmcp_auth::MAX_HANDLE_LENGTH,
        "the test handle must satisfy the compiled default so only the narrowed \
         override, not the default, can be responsible for a rejection"
    );

    let addr = start_server_with_config(
        common::TestServerConfigBuilder::new(tempfile::tempdir().unwrap().keep())
            .token_key([43u8; 32])
            .max_handle_length(NARROWED_MAX_HANDLE_LENGTH)
            .build(),
    )
    .await;
    let resp = reqwest::Client::new()
        .post(format!("http://{addr}/auth/register"))
        .json(&serde_json::json!({
            "handle": handle,
            "password": "validpassword"
        }))
        .send()
        .await
        .expect("register");
    assert_eq!(
        resp.status(),
        400,
        "a handle under the compiled default but over the config-resolved \
         max_handle_length must be rejected by the live HTTP endpoint"
    );
}
