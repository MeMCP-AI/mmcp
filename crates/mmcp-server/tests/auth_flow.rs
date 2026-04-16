//! Integration test: register + login flow.

use std::net::SocketAddr;

async fn start_server() -> SocketAddr {
    let cfg = mmcp_server::config::ServerConfig {
        bind: "127.0.0.1:0".parse().unwrap(),
        database_url: "sqlite::memory:".to_string(),
        repo_root: tempfile::tempdir().unwrap().keep(),
        token_key: [42u8; 32],
        oauth_providers: vec![],
        origin: "http://localhost:8787".to_string(),
    };
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
            "password": "hunter2"
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
            "password": "hunter2"
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
            "password": "correct"
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
        "password": "pass"
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
