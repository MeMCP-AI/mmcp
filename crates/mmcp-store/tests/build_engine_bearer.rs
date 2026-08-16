#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Proves `build_engine` wires its `token` argument to the
//! `/sync/*` `Authorization` header and its `push_token` argument to
//! the git content-plane credential, and that the two never leak
//! into each other's slot. Mirrors the wiremock pattern in
//! `crates/mmcp-sync/tests/engine_smoke.rs`.

use std::sync::Arc;

use mmcp_store::testing::ScratchHome;
use mmcp_store::{IndexResolver, build_engine};
use mmcp_sync::{ManifestResponse, SyncFilter};
use wiremock::matchers::{header, method, path};
use wiremock::{Match, Mock, MockServer, Request, ResponseTemplate};

const BEARER_TOKEN: &str = "test-bearer-token-xyz";
const PUSH_TOKEN: &str = "test-push-credential-abc";

/// Matches only a request that carries no `authorization` header at
/// all. Wiremock ships matchers for an exact or present header, but
/// none for absence, so this fills that gap.
struct NoAuthorizationHeader;

impl Match for NoAuthorizationHeader {
    fn matches(&self, request: &Request) -> bool {
        !request.headers.contains_key("authorization")
    }
}

/// Build an engine through `build_engine` and run a `fetch` against
/// a mocked `/sync/manifest` endpoint, so every test below drives
/// the exact same call path a real CLI/GUI/MCP-tool caller does.
/// Returns the built engine so callers can additionally inspect its
/// content-plane `git_credentials`.
async fn fetch_through_build_engine(
    server_uri: &str,
    token: Option<&str>,
    push_token: Option<&str>,
) -> mmcp_sync::SyncEngine {
    let home = ScratchHome::new().await.expect("scratch home");
    let (engine, resolver): (_, IndexResolver) = build_engine(
        Arc::clone(home.backend()),
        home.groups().clone(),
        server_uri,
        token,
        push_token,
    )
    .expect("build engine");

    let report = engine
        .fetch(SyncFilter::All, &resolver, &resolver)
        .await
        .expect("fetch against the mocked manifest endpoint must succeed");
    assert!(report.groups.is_empty());
    assert!(report.new_groups.is_empty());
    engine
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

    fetch_through_build_engine(&server.uri(), Some(BEARER_TOKEN), None).await;
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

    fetch_through_build_engine(&server.uri(), None, None).await;
}

/// Configuring only `push_token` must never send it as the
/// `/sync/*` `Authorization` header: the control plane stays
/// untokened even though the content-plane credential is set.
#[tokio::test]
async fn build_engine_with_only_a_push_token_sends_no_authorization_header() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/sync/manifest"))
        .and(NoAuthorizationHeader)
        .respond_with(ResponseTemplate::new(200).set_body_json(ManifestResponse { groups: vec![] }))
        .expect(1)
        .mount(&server)
        .await;

    let engine = fetch_through_build_engine(&server.uri(), None, Some(PUSH_TOKEN)).await;
    assert_eq!(
        engine.git_credentials(),
        mmcp_git::Credentials::bearer(PUSH_TOKEN),
        "the push token must still reach git_credentials despite sending no Authorization header"
    );
}

/// Configuring both credentials with DIFFERENT values proves
/// `build_engine` keeps them in separate slots: the control-plane
/// bearer header carries `token`, the content-plane credential
/// carries `push_token`, and neither overwrites the other.
#[tokio::test]
async fn build_engine_with_both_tokens_keeps_each_plane_independent() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/sync/manifest"))
        .and(header("authorization", format!("Bearer {BEARER_TOKEN}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(ManifestResponse { groups: vec![] }))
        .expect(1)
        .mount(&server)
        .await;

    let engine =
        fetch_through_build_engine(&server.uri(), Some(BEARER_TOKEN), Some(PUSH_TOKEN)).await;
    assert_eq!(
        engine.git_credentials(),
        mmcp_git::Credentials::bearer(PUSH_TOKEN),
        "git_credentials must carry push_token, never the control-plane bearer token"
    );
}
