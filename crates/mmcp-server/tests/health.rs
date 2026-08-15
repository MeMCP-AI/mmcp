//! Integration test: health endpoint.

use std::net::SocketAddr;

mod common;

/// Start the server on an ephemeral port and return its address.
async fn start_server() -> SocketAddr {
    let cfg = common::TestServerConfigBuilder::new(tempfile::tempdir().unwrap().keep()).build();
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
