#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Integration test: the unauthenticated auth routes share one per-client budget.

use std::net::SocketAddr;

use reqwest::{Client, Method, Response, StatusCode};

mod common;

/// Refill rate of every test server.
/// One request per minute, so the budget never refills while a test runs.
const PER_MINUTE: u32 = 1;
/// The loopback address every test client connects from.
const LOOPBACK_PROXY_RANGE: &str = "127.0.0.1/32";
const ALLOWED_ORIGIN: &str = "http://127.0.0.1:3000";

async fn start_server(burst: u32) -> SocketAddr {
    start_configured_server(
        common::TestServerConfigBuilder::new(tempfile::tempdir().unwrap().keep()),
        burst,
    )
    .await
}

async fn start_configured_server(
    builder: common::TestServerConfigBuilder,
    burst: u32,
) -> SocketAddr {
    common::start_server_with_config(builder.auth_rate_limit(PER_MINUTE, burst).build()).await
}

async fn wrong_password_login(client: &Client, addr: SocketAddr) -> Response {
    client
        .post(format!("http://{addr}/auth/login"))
        .json(&serde_json::json!({ "handle": "nobody", "password": "wrong-password" }))
        .send()
        .await
        .unwrap()
}

async fn login_from_forwarded(client: &Client, addr: SocketAddr, forwarded: &str) -> Response {
    client
        .post(format!("http://{addr}/auth/login"))
        .header("x-forwarded-for", forwarded)
        .json(&serde_json::json!({ "handle": "nobody", "password": "wrong-password" }))
        .send()
        .await
        .unwrap()
}

fn retry_after_seconds(response: &Response) -> u64 {
    response
        .headers()
        .get("retry-after")
        .expect("a 429 carries Retry-After")
        .to_str()
        .unwrap()
        .parse()
        .expect("Retry-After is delay-seconds")
}

#[tokio::test]
async fn login_beyond_the_burst_answers_429_with_retry_after() {
    const BURST: u32 = 3;
    let addr = start_server(BURST).await;
    let client = Client::new();

    for _ in 0..BURST {
        assert_eq!(
            wrong_password_login(&client, addr).await.status(),
            StatusCode::UNAUTHORIZED
        );
    }
    let refused = wrong_password_login(&client, addr).await;

    assert_eq!(refused.status(), StatusCode::TOO_MANY_REQUESTS);
    let retry_after = retry_after_seconds(&refused);
    assert!(retry_after >= 1, "Retry-After is at least one second");
    assert_eq!(
        refused.text().await.unwrap(),
        format!("too many requests; retry after {retry_after} seconds")
    );
}

#[tokio::test]
async fn one_budget_is_shared_across_the_unauthenticated_auth_routes() {
    const BURST: u32 = 4;
    let addr = start_server(BURST).await;
    let client = Client::new();

    // Four requests on four different unauthenticated routes spend the whole burst.
    let register = client
        .post(format!("http://{addr}/auth/register"))
        .json(&serde_json::json!({ "handle": "alice", "password": "hunter22" }))
        .send()
        .await
        .unwrap();
    assert_eq!(register.status(), StatusCode::CREATED);
    assert_eq!(
        wrong_password_login(&client, addr).await.status(),
        StatusCode::UNAUTHORIZED
    );
    let authorize = client
        .get(format!("http://{addr}/auth/oauth/github/authorize"))
        .send()
        .await
        .unwrap();
    assert_eq!(authorize.status(), StatusCode::NOT_FOUND);
    let passkey_start = client
        .post(format!("http://{addr}/auth/passkey/login/start"))
        .json(&serde_json::json!({ "handle": "alice" }))
        .send()
        .await
        .unwrap();
    assert_ne!(passkey_start.status(), StatusCode::TOO_MANY_REQUESTS);

    // Every route is refused now, whichever one the budget was spent on.
    assert_eq!(
        wrong_password_login(&client, addr).await.status(),
        StatusCode::TOO_MANY_REQUESTS
    );
    let refused_register = client
        .post(format!("http://{addr}/auth/register"))
        .json(&serde_json::json!({ "handle": "bob", "password": "hunter22" }))
        .send()
        .await
        .unwrap();
    assert_eq!(refused_register.status(), StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test]
async fn refusal_is_answered_before_the_request_body_is_read() {
    let addr = start_server(1).await;
    let client = Client::new();
    assert_eq!(
        wrong_password_login(&client, addr).await.status(),
        StatusCode::UNAUTHORIZED
    );

    // Without the limiter this body is a 4xx from the JSON extractor, so a 429 proves the check ran first.
    let refused = client
        .post(format!("http://{addr}/auth/login"))
        .header("content-type", "application/json")
        .body("this is not json")
        .send()
        .await
        .unwrap();

    assert_eq!(refused.status(), StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test]
async fn authenticated_and_unmatched_auth_paths_are_not_counted() {
    let addr = start_server(1).await;
    let client = Client::new();
    assert_eq!(
        wrong_password_login(&client, addr).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        wrong_password_login(&client, addr).await.status(),
        StatusCode::TOO_MANY_REQUESTS,
        "the budget is spent"
    );

    let health = client
        .get(format!("http://{addr}/health"))
        .send()
        .await
        .unwrap();
    assert_eq!(health.status(), StatusCode::OK);
    // A session-required route answers its own 401, never the limiter's 429.
    let register_start = client
        .post(format!("http://{addr}/auth/passkey/register/start"))
        .send()
        .await
        .unwrap();
    assert_eq!(register_start.status(), StatusCode::UNAUTHORIZED);
    let unknown = client
        .get(format!("http://{addr}/auth/no-such-route"))
        .send()
        .await
        .unwrap();
    assert_eq!(unknown.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn cors_preflight_is_not_counted() {
    let addr = start_configured_server(
        common::TestServerConfigBuilder::new(tempfile::tempdir().unwrap().keep())
            .cors_allowed_origins(&[ALLOWED_ORIGIN]),
        1,
    )
    .await;
    let client = Client::new();
    assert_eq!(
        wrong_password_login(&client, addr).await.status(),
        StatusCode::UNAUTHORIZED
    );

    let preflight = client
        .request(Method::OPTIONS, format!("http://{addr}/auth/login"))
        .header("origin", ALLOWED_ORIGIN)
        .header("access-control-request-method", "POST")
        .header("access-control-request-headers", "content-type")
        .send()
        .await
        .unwrap();

    assert_ne!(preflight.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(
        preflight
            .headers()
            .get("access-control-allow-origin")
            .map(|value| value.to_str().unwrap()),
        Some(ALLOWED_ORIGIN)
    );
}

#[tokio::test]
async fn clients_behind_a_trusted_proxy_have_separate_budgets() {
    let addr = start_configured_server(
        common::TestServerConfigBuilder::new(tempfile::tempdir().unwrap().keep())
            .trusted_proxies(LOOPBACK_PROXY_RANGE),
        1,
    )
    .await;
    let client = Client::new();

    assert_eq!(
        login_from_forwarded(&client, addr, "203.0.113.7")
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        login_from_forwarded(&client, addr, "203.0.113.7")
            .await
            .status(),
        StatusCode::TOO_MANY_REQUESTS,
        "the first client spent its own budget"
    );
    assert_eq!(
        login_from_forwarded(&client, addr, "203.0.113.8")
            .await
            .status(),
        StatusCode::UNAUTHORIZED,
        "the second client has a budget of its own"
    );
}

#[tokio::test]
async fn x_forwarded_for_from_a_direct_client_does_not_renew_its_budget() {
    // No trusted proxy is configured, so the loopback client is a direct client.
    let addr = start_server(1).await;
    let client = Client::new();

    assert_eq!(
        login_from_forwarded(&client, addr, "203.0.113.7")
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        login_from_forwarded(&client, addr, "203.0.113.8")
            .await
            .status(),
        StatusCode::TOO_MANY_REQUESTS,
        "a client choosing its own forwarded address keeps one budget"
    );
}

#[tokio::test]
async fn server_served_without_client_addresses_fails_closed() {
    let cfg = common::TestServerConfigBuilder::new(tempfile::tempdir().unwrap().keep())
        .auth_rate_limit(PER_MINUTE, 1)
        .build();
    let state = mmcp_server::state::ServerState::initialize(&cfg)
        .await
        .unwrap();
    let app = mmcp_server::app::build_router(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    // Served on purpose without `serve_router`, so no request carries a peer address.
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let response = wrong_password_login(&Client::new(), addr).await;

    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(response.text().await.unwrap(), "internal server error");
}
