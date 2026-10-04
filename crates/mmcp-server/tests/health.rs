#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Integration test: health endpoint.

use std::net::SocketAddr;

mod common;

/// Start the server on an ephemeral port and return its address.
async fn start_server() -> SocketAddr {
    let cfg = common::TestServerConfigBuilder::new(tempfile::tempdir().unwrap().keep()).build();
    common::start_server_with_config(cfg).await
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
