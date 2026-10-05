#![allow(clippy::unwrap_used, clippy::expect_used)]
//! `GET /sync/manifest` on a database connection that SCRAM refuses after the first one.
//!
//! A pool opens a new connection whenever it has none idle, and the driver error of a SASLprep failure on that connection quotes the character.
//! The route reads the groups through mmcp-db, so neither its response nor any log line may hold that error.

use std::sync::Arc;

use mmcp_auth::SessionClaims;
use mmcp_db::connection::fake_peer::{Behavior, FakePeer, SASLPREP_FAILURE_MARKERS};
use mmcp_db::connection::log_capture::LogCapture;
use mmcp_db::repository::user_repo;
use mmcp_server::state::ServerState;
use tempfile::TempDir;
use uuid::Uuid;

mod common;

const SENTINEL: &str = "SENTINEL_MANIFEST_PASSWORD";

/// Lifetime, in seconds, of the token minted for the request.
const TOKEN_LIFETIME_SECS: i64 = 3600;

#[tokio::test]
async fn a_manifest_request_on_a_connection_that_scram_refuses_leaks_no_credential_text() {
    let (capture, _guard) = LogCapture::install();
    let peer = FakePeer::start(Behavior::TrustsFirstThenRequestsScram).await;
    let tmp = TempDir::new().expect("tempdir");
    let cfg = common::TestServerConfigBuilder::new(tmp.path().to_path_buf()).build();
    let state = ServerState::initialize(&cfg).await.expect("server init");

    // The bearer token is verified without the database, but the user row keeps the token real.
    let user_id = Uuid::now_v7();
    user_repo::create(
        state.database.connection(),
        user_repo::NewUser {
            id: user_id,
            handle: "alice".to_string(),
            display_name: None,
            password_hash: None,
            email: None,
            created_at: jiff::Timestamp::now().as_millisecond(),
        },
    )
    .await
    .expect("seed the user");
    let claims = SessionClaims::new_with_lifetime(
        user_id,
        Uuid::now_v7(),
        jiff::Timestamp::now().as_second(),
        TOKEN_LIFETIME_SECS,
    );
    let token = state.token_issuer.issue(&claims).expect("issue token");

    // The groups are read from the peer's database, whose first connection the pool finds closed.
    let target = format!(
        "postgres://mmcp:p%F0%9F%98%80{SENTINEL}@127.0.0.1:{}/app?sslmode=disable",
        peer.port()
    )
    .parse()
    .expect("the URL is accepted");
    let database = mmcp_db::connect(&target)
        .await
        .expect("the first connection needs no password");
    let mut inner = Arc::try_unwrap(state.0).unwrap_or_else(|_| panic!("one owner of the state"));
    inner.database = database;
    let app = mmcp_server::app::build_router(ServerState(Arc::new(inner)));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("local addr");
    tokio::spawn(async move {
        mmcp_server::serve::serve_router(listener, app)
            .await
            .unwrap();
    });

    let response = reqwest::Client::new()
        .get(format!("http://{addr}/sync/manifest"))
        .bearer_auth(&token)
        .send()
        .await
        .expect("GET manifest");

    assert_eq!(response.status(), 500);
    let body = response.text().await.expect("read body");
    let json: serde_json::Value = serde_json::from_str(&body).expect("decode json");
    assert_eq!(
        json["error"]["message"],
        mmcp_server::routes::response::GENERIC_INTERNAL_ERROR_MESSAGE,
        "{body}"
    );
    for marker in SASLPREP_FAILURE_MARKERS.iter().chain(&[SENTINEL]) {
        assert!(!body.contains(marker), "the response holds `{marker}`");
    }
    capture.assert_holds_none_of(&[SENTINEL]);
    capture.assert_holds_none_of(&SASLPREP_FAILURE_MARKERS);
}
