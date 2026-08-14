//! Integration test: health endpoint.

use std::net::SocketAddr;

/// Start the server on an ephemeral port and return its address.
async fn start_server() -> SocketAddr {
    let cfg = mmcp_server::config::ServerConfig {
        bind: "127.0.0.1:0".parse().unwrap(),
        database_url: "sqlite::memory:".to_string(),
        repo_root: tempfile::tempdir().unwrap().keep(),
        token_key: [0u8; 32],
        oauth_providers: vec![],
        origin: "http://localhost:8787".to_string(),
        push_token: None,
        min_password_length: mmcp_auth::MIN_PASSWORD_LENGTH,
        max_password_length: mmcp_auth::MAX_PASSWORD_LENGTH,
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
async fn health_returns_ok() {
    let addr = start_server().await;
    let url = format!("http://{addr}/health");
    let resp = reqwest::get(&url).await.expect("GET /health");
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.expect("json");
    assert_eq!(body["status"], "ok");
    assert_eq!(body["name"], "mmcp-server");
}
