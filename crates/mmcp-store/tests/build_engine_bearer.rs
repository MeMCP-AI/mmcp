#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Proves `build_engine_with_env` wires a legacy `server_url`
//! shorthand remote's env-derived `token` to the `/sync/*`
//! `Authorization` header and its `push_token` to the git
//! content-plane credential, and that the two never leak into each
//! other's slot. Mirrors the wiremock pattern in
//! `crates/mmcp-sync/tests/engine_smoke.rs`.
//!
//! Uses `build_engine_with_env`, not `build_engine`, throughout: the
//! injectable environment lookup keeps every case here free of a
//! real `std::env::var` race under cargo's parallel test runner (see
//! `mmcp_core::config::SyncConfig::resolve_token_with`'s doc comment
//! for the same rationale applied one layer down).

use std::sync::Arc;

use mmcp_core::config::{ProjectConfig, Remote, SyncConfig, UserConfig};
use mmcp_core::id::ProjectUuid;
use mmcp_store::testing::ScratchHome;
use mmcp_store::{IndexResolver, build_engine_with_env, resolve_effective_remotes};
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

/// A project config declaring only the legacy `server_url` shorthand
/// pointed at `server_uri`, so the resolver synthesizes a single
/// `mmcp-server` remote and it is the implicit default.
fn project_config_with_legacy_server_url(server_uri: &str) -> ProjectConfig {
    ProjectConfig {
        project_uuid: ProjectUuid::new(),
        project_slug: None,
        sync: SyncConfig {
            server_url: Some(server_uri.to_string()),
            remotes: Vec::new(),
        },
        project_remote_only: false,
        subscriptions: mmcp_core::config::SubscriptionsConfig::default(),
    }
}

/// Build an engine through `build_engine_with_env` and run a `fetch`
/// against a mocked `/sync/manifest` endpoint, so every test below
/// drives the exact same call path a real CLI/GUI/MCP-tool caller
/// does. Returns the built engine so callers can additionally
/// inspect its content-plane `git_credentials`.
async fn fetch_through_build_engine(
    server_uri: &str,
    token: Option<&'static str>,
    push_token: Option<&'static str>,
) -> mmcp_sync::SyncEngine {
    let home = ScratchHome::new().await.expect("scratch home");
    let project = project_config_with_legacy_server_url(server_uri);
    let effective = resolve_effective_remotes(&UserConfig::default(), &project)
        .expect("resolve effective remotes");

    let (engine, resolver): (_, IndexResolver) = build_engine_with_env(
        Arc::clone(home.backend()),
        home.groups().clone(),
        &effective,
        move |key| match key {
            "MMCP_SYNC_TOKEN" => token.map(str::to_string),
            "MMCP_SYNC_PUSH_TOKEN" => push_token.map(str::to_string),
            _ => None,
        },
    )
    .await
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
        engine.remotes()[0].git_credentials(),
        mmcp_git::Credentials::bearer(PUSH_TOKEN),
        "the push token must still reach git_credentials despite sending no Authorization header"
    );
}

/// Configuring both credentials with DIFFERENT values proves
/// `build_engine_with_env` keeps them in separate slots: the
/// control-plane bearer header carries `token`, the content-plane
/// credential carries `push_token`, and neither overwrites the
/// other.
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
        engine.remotes()[0].git_credentials(),
        mmcp_git::Credentials::bearer(PUSH_TOKEN),
        "git_credentials must carry push_token, never the control-plane bearer token"
    );
}

/// A `direct-git`-only project (no legacy shorthand, no `default`
/// flag needed since it is the sole remote) resolves through the
/// same `build_engine_with_env` path once its declared `group`
/// matches a group the local mirror actually has.
#[tokio::test]
async fn build_engine_resolves_a_direct_git_remotes_group_against_the_local_mirror() {
    let home = ScratchHome::new().await.expect("scratch home");
    let seeded = home.seed_group("team-mirror").await.expect("seed group");

    let project = ProjectConfig {
        project_uuid: ProjectUuid::new(),
        project_slug: None,
        sync: SyncConfig {
            server_url: None,
            remotes: vec![Remote::DirectGit {
                name: "mirror".to_string(),
                url: "ssh://git@example.com/mirror.git".to_string(),
                auth: mmcp_core::config::RemoteAuth::None,
                group: Some(seeded.group_id.to_string()),
                default: false,
                include_in_push_all: true,
            }],
        },
        project_remote_only: false,
        subscriptions: mmcp_core::config::SubscriptionsConfig::default(),
    };
    let effective =
        resolve_effective_remotes(&UserConfig::default(), &project).expect("resolve remotes");

    let (engine, _resolver): (_, IndexResolver) = build_engine_with_env(
        Arc::clone(home.backend()),
        home.groups().clone(),
        &effective,
        |_| None,
    )
    .await
    .expect("a direct-git remote whose group matches a real local group must build");

    assert_eq!(engine.remotes().len(), 1);
    assert_eq!(engine.remotes()[0].name, "mirror");
    // The sole remote is the implicit default per the resolver's
    // rule (c): exactly one remote in the effective set.
    assert!(engine.remotes()[0].default);
}
