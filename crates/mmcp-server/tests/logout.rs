#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Integration test: logout ends the server session the login created.

use std::net::SocketAddr;

use reqwest::{Client, StatusCode};

mod common;

const HANDLE: &str = "alice";
const PASSWORD: &str = "hunter22";

async fn start_server() -> SocketAddr {
    common::start_server_with_config(
        common::TestServerConfigBuilder::new(tempfile::tempdir().unwrap().keep())
            .token_key([42u8; 32])
            .build(),
    )
    .await
}

/// A client that keeps the session cookie, logged in as the test account.
async fn logged_in_client(addr: SocketAddr) -> Client {
    let client = Client::builder().cookie_store(true).build().unwrap();
    let register = client
        .post(format!("http://{addr}/auth/register"))
        .json(&serde_json::json!({ "handle": HANDLE, "password": PASSWORD }))
        .send()
        .await
        .unwrap();
    assert_eq!(register.status(), StatusCode::CREATED);
    let login = client
        .post(format!("http://{addr}/auth/login"))
        .json(&serde_json::json!({ "handle": HANDLE, "password": PASSWORD }))
        .send()
        .await
        .unwrap();
    assert_eq!(login.status(), StatusCode::OK);
    client
}

/// A route only a session authorizes: it starts a passkey enrollment on the caller's account.
async fn session_status(client: &Client, addr: SocketAddr) -> StatusCode {
    client
        .post(format!("http://{addr}/auth/passkey/register/start"))
        .send()
        .await
        .unwrap()
        .status()
}

async fn logout(client: &Client, addr: SocketAddr) -> StatusCode {
    client
        .post(format!("http://{addr}/auth/logout"))
        .send()
        .await
        .unwrap()
        .status()
}

#[tokio::test]
async fn logout_ends_the_session_that_login_created() {
    let addr = start_server().await;
    let client = logged_in_client(addr).await;
    assert_eq!(
        session_status(&client, addr).await,
        StatusCode::OK,
        "the login cookie authorizes the session route"
    );

    assert_eq!(logout(&client, addr).await, StatusCode::NO_CONTENT);

    assert_eq!(
        session_status(&client, addr).await,
        StatusCode::UNAUTHORIZED,
        "the cookie no longer authorizes anything"
    );
}

#[tokio::test]
async fn logout_is_idempotent_and_needs_no_session() {
    let addr = start_server().await;
    let client = logged_in_client(addr).await;
    assert_eq!(logout(&client, addr).await, StatusCode::NO_CONTENT);
    assert_eq!(logout(&client, addr).await, StatusCode::NO_CONTENT);

    let without_cookie = Client::new();
    assert_eq!(logout(&without_cookie, addr).await, StatusCode::NO_CONTENT);
}
