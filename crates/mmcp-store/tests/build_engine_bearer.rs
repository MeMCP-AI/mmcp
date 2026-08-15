//! Proves `build_engine` wires its `token` argument all the way to
//! the `Authorization` header on the wire, mirroring the wiremock
//! pattern in `crates/mmcp-sync/tests/engine_smoke.rs`.

use std::sync::Arc;

use mmcp_store::testing::ScratchHome;
use mmcp_store::{IndexResolver, build_engine};
use mmcp_sync::{ManifestResponse, SyncFilter};
use wiremock::matchers::{header, method, path};
use wiremock::{Match, Mock, MockServer, Request, ResponseTemplate};

const BEARER_TOKEN: &str = "test-bearer-token-xyz";

/// Matches only a request that carries no `authorization` header at
/// all. Wiremock ships matchers for an exact or present header, but
/// none for absence, so this fills that gap.
struct NoAuthorizationHeader;

impl Match for NoAuthorizationHeader {
    fn matches(&self, request: &Request) -> bool {
        !request.headers.contains_key("authorization")
    }
}

/// Run a `fetch` against a mocked `/sync/manifest` endpoint through
/// `build_engine`, so both tests below drive the exact same call
/// path a real CLI/GUI/MCP-tool caller does.
async fn fetch_through_build_engine(server_uri: &str, token: Option<&str>) {
    let home = ScratchHome::new().await.expect("scratch home");
    let (engine, resolver): (_, IndexResolver) = build_engine(
        Arc::clone(home.backend()),
        home.groups().clone(),
        server_uri,
        token,
    )
    .expect("build engine");

    let report = engine
        .fetch(SyncFilter::All, &resolver, &resolver)
        .await
        .expect("fetch against the mocked manifest endpoint must succeed");
    assert!(report.groups.is_empty());
    assert!(report.new_groups.is_empty());
}

#[tokio::test]
async fn build_engine_with_a_token_sends_the_bearer_header() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/sync/manifest"))
        .and(header("authorization", format!("Bearer {BEARER_TOKEN}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(ManifestResponse { groups: vec![] }))
        .expect(1)
        .mount(&server)
        .await;

    fetch_through_build_engine(&server.uri(), Some(BEARER_TOKEN)).await;
}

#[tokio::test]
async fn build_engine_without_a_token_sends_no_authorization_header() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/sync/manifest"))
        .and(NoAuthorizationHeader)
        .respond_with(ResponseTemplate::new(200).set_body_json(ManifestResponse { groups: vec![] }))
        .expect(1)
        .mount(&server)
        .await;

    fetch_through_build_engine(&server.uri(), None).await;
}
