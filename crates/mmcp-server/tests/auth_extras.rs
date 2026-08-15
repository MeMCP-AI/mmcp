//! Error-branch coverage for `/auth/*` routes.
//!
//! `auth_flow.rs` covers the password happy path and the wrong-password/duplicate-handle 4xx cases.
//! This suite covers the OAuth branches, the passkey start/finish error paths, and the unknown-handle login path:
//! each test targets the server-local error surface, without a real WebAuthn client or a live OAuth provider.

use std::net::SocketAddr;

use axum::{
    Json, Router,
    routing::{get, post},
};
use mmcp_server::config::OAuthProviderConfig;
use mmcp_server::state::ServerState;
use serde_json::json;
use tempfile::TempDir;

mod common;

/// Bootstrap the server with the caller's choice of OAuth providers.
/// The listener is an ephemeral port on loopback; nothing touches
/// the real filesystem beyond a tempdir repo root.
async fn start_server_with_oauth(providers: Vec<OAuthProviderConfig>) -> (SocketAddr, TempDir) {
    let tmp = TempDir::new().expect("tempdir");
    let cfg = common::TestServerConfigBuilder::new(tmp.path().to_path_buf())
        .token_key([7u8; 32])
        .oauth_providers(providers)
        .build();
    let state = ServerState::initialize(&cfg).await.expect("state init");
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
    (addr, tmp)
}

fn github_provider() -> OAuthProviderConfig {
    OAuthProviderConfig::github("client-abc", "secret-xyz")
}

// ── Password: unknown handle ────────────────────────────────────────

#[tokio::test]
async fn login_with_unknown_handle_returns_401() {
    let (addr, _tmp) = start_server_with_oauth(vec![]).await;
    let resp = reqwest::Client::new()
        .post(format!("http://{addr}/auth/login"))
        .json(&json!({ "handle": "ghost", "password": "whatever" }))
        .send()
        .await
        .expect("login");
    assert_eq!(resp.status(), 401);
}

// ── OAuth authorize ─────────────────────────────────────────────────

#[tokio::test]
async fn oauth_authorize_unknown_provider_returns_404() {
    let (addr, _tmp) = start_server_with_oauth(vec![]).await;
    let resp = reqwest::Client::new()
        .get(format!("http://{addr}/auth/oauth/google/authorize"))
        .send()
        .await
        .expect("oauth authorize");
    assert_eq!(resp.status(), 404);
}

#[tokio::test]
async fn oauth_authorize_known_provider_redirects_to_provider_authorize_url() {
    let (addr, _tmp) = start_server_with_oauth(vec![github_provider()]).await;
    // Disable auto-follow so we can inspect the 302.
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("build client");
    let resp = client
        .get(format!("http://{addr}/auth/oauth/github/authorize"))
        .send()
        .await
        .expect("oauth authorize");

    assert!(
        resp.status().is_redirection(),
        "expected redirect, got {}",
        resp.status()
    );
    let location = resp
        .headers()
        .get(reqwest::header::LOCATION)
        .expect("Location header")
        .to_str()
        .expect("utf-8");
    assert!(
        location.starts_with("https://github.com/login/oauth/authorize"),
        "unexpected redirect target: {location}"
    );
    assert!(location.contains("client_id=client-abc"));
    // The callback URL round-trips through `urlencoding::encode`
    // before being appended; ensure it points back at our origin.
    assert!(location.contains("redirect_uri="));
    // The handler appends the scope literally (no URL-encoding on
    // the `:` since it is a reserved char that is legal in a query).
    assert!(location.contains("scope=user:email"));
}

// ── OAuth CSRF state (mmcp issue #198) ──────────────────────────────

/// Extracts the raw value of a `state` query parameter from an `oauth_authorize`
/// redirect `Location` header.
fn extract_state_param(location: &str) -> String {
    location
        .split("state=")
        .nth(1)
        .expect("redirect Location must carry a state= query parameter")
        .split('&')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// Spins up a minimal fake OAuth provider (token exchange + userinfo) on an
/// ephemeral loopback port, so the callback happy path can be proven end to
/// end without a live GitHub dependency.
async fn start_fake_oauth_provider() -> SocketAddr {
    let app = Router::new()
        .route(
            "/token",
            post(|| async { Json(json!({ "access_token": "fake-access-token" })) }),
        )
        .route(
            "/userinfo",
            get(|| async {
                Json(json!({ "id": 42, "login": "octocat", "email": "octocat@example.com" }))
            }),
        );
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
async fn oauth_authorize_redirect_includes_a_nonempty_state_parameter() {
    let (addr, _tmp) = start_server_with_oauth(vec![github_provider()]).await;
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("build client");

    let resp = client
        .get(format!("http://{addr}/auth/oauth/github/authorize"))
        .send()
        .await
        .expect("oauth authorize");
    let location = resp
        .headers()
        .get(reqwest::header::LOCATION)
        .expect("Location header")
        .to_str()
        .expect("utf-8");

    let state_value = extract_state_param(location);
    assert!(
        !state_value.is_empty(),
        "state parameter must not be empty, got redirect: {location}"
    );
}

#[tokio::test]
async fn oauth_callback_with_matching_state_completes_the_login() {
    let fake_addr = start_fake_oauth_provider().await;
    let provider = OAuthProviderConfig {
        slug: "github".to_string(),
        client_id: "client-abc".to_string(),
        client_secret: "secret-xyz".to_string(),
        auth_url: format!("http://{fake_addr}/authorize"),
        token_url: format!("http://{fake_addr}/token"),
        userinfo_url: format!("http://{fake_addr}/userinfo"),
    };
    let (addr, _tmp) = start_server_with_oauth(vec![provider]).await;
    let client = reqwest::Client::builder()
        .cookie_store(true)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("build client");

    let authorize_resp = client
        .get(format!("http://{addr}/auth/oauth/github/authorize"))
        .send()
        .await
        .expect("oauth authorize");
    let location = authorize_resp
        .headers()
        .get(reqwest::header::LOCATION)
        .expect("Location header")
        .to_str()
        .expect("utf-8")
        .to_string();
    let state = extract_state_param(&location);

    let callback_resp = client
        .get(format!(
            "http://{addr}/auth/oauth/github/callback?code=fake-code&state={state}"
        ))
        .send()
        .await
        .expect("oauth callback");

    let status = callback_resp.status();
    let body = callback_resp.text().await.unwrap_or_default();
    assert_eq!(
        status, 200,
        "matching state must complete the oauth login, got body: {body}"
    );
    assert!(body.contains("OAuth login successful"));
}

#[tokio::test]
async fn oauth_callback_with_mismatched_state_is_rejected_before_token_exchange() {
    // Nothing listens on this loopback port: a live token-exchange call
    // would fail loudly (connection refused), surfacing as a 500 through
    // `into_generic_response` rather than the state check's 400. A 400
    // response therefore proves the exchange was never attempted.
    let unreachable_provider = OAuthProviderConfig {
        slug: "github".to_string(),
        client_id: "client-abc".to_string(),
        client_secret: "secret-xyz".to_string(),
        auth_url: "https://github.com/login/oauth/authorize".to_string(),
        token_url: "http://127.0.0.1:1/oauth/token".to_string(),
        userinfo_url: "http://127.0.0.1:1/oauth/userinfo".to_string(),
    };
    let (addr, _tmp) = start_server_with_oauth(vec![unreachable_provider]).await;
    let client = reqwest::Client::builder()
        .cookie_store(true)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("build client");

    // Establish a session (and its stored state) via authorize, then send a
    // callback carrying a state that does not match it.
    client
        .get(format!("http://{addr}/auth/oauth/github/authorize"))
        .send()
        .await
        .expect("oauth authorize");

    let resp = client
        .get(format!(
            "http://{addr}/auth/oauth/github/callback?code=fake-code&state=not-the-real-state"
        ))
        .send()
        .await
        .expect("oauth callback");

    assert_eq!(
        resp.status(),
        400,
        "mismatched state must be rejected before any token-exchange call fires"
    );
}

#[tokio::test]
async fn oauth_callback_with_missing_state_is_rejected_before_token_exchange() {
    let unreachable_provider = OAuthProviderConfig {
        slug: "github".to_string(),
        client_id: "client-abc".to_string(),
        client_secret: "secret-xyz".to_string(),
        auth_url: "https://github.com/login/oauth/authorize".to_string(),
        token_url: "http://127.0.0.1:1/oauth/token".to_string(),
        userinfo_url: "http://127.0.0.1:1/oauth/userinfo".to_string(),
    };
    let (addr, _tmp) = start_server_with_oauth(vec![unreachable_provider]).await;
    let client = reqwest::Client::builder()
        .cookie_store(true)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("build client");

    client
        .get(format!("http://{addr}/auth/oauth/github/authorize"))
        .send()
        .await
        .expect("oauth authorize");

    let resp = client
        .get(format!(
            "http://{addr}/auth/oauth/github/callback?code=fake-code"
        ))
        .send()
        .await
        .expect("oauth callback");

    assert_eq!(
        resp.status(),
        400,
        "missing state must be rejected before any token-exchange call fires"
    );
}

// ── OAuth session cookie (SameSite=Lax survives the provider's cross-site redirect) ──

#[tokio::test]
async fn oauth_authorize_sets_a_samesite_lax_cookie() {
    let (addr, _tmp) = start_server_with_oauth(vec![github_provider()]).await;
    // Disable auto-follow: a followed redirect would inspect
    // github.com's own response cookies instead of the Set-Cookie
    // this server's own authorize handler just emitted.
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("build client");
    let resp = client
        .get(format!("http://{addr}/auth/oauth/github/authorize"))
        .send()
        .await
        .expect("oauth authorize");

    let set_cookie_headers: Vec<String> = resp
        .headers()
        .get_all(reqwest::header::SET_COOKIE)
        .iter()
        .map(|v| v.to_str().expect("utf-8 Set-Cookie header").to_lowercase())
        .collect();
    assert!(
        set_cookie_headers
            .iter()
            .any(|c| c.contains("samesite=lax")),
        "authorize must set the session cookie with SameSite=Lax so it survives the OAuth \
         provider's cross-site top-level redirect back to the callback, got: \
         {set_cookie_headers:?}"
    );
}

// ── OAuth state rejection causes take independent code paths ───────

#[tokio::test]
async fn oauth_callback_with_no_stored_state_is_rejected() {
    // No prior `authorize` call: the session carries no stored state
    // at all, distinct from a callback query that omits `state`
    // entirely
    // (`oauth_callback_with_missing_state_is_rejected_before_token_exchange`).
    let unreachable_provider = OAuthProviderConfig {
        slug: "github".to_string(),
        client_id: "client-abc".to_string(),
        client_secret: "secret-xyz".to_string(),
        auth_url: "https://github.com/login/oauth/authorize".to_string(),
        token_url: "http://127.0.0.1:1/oauth/token".to_string(),
        userinfo_url: "http://127.0.0.1:1/oauth/userinfo".to_string(),
    };
    let (addr, _tmp) = start_server_with_oauth(vec![unreachable_provider]).await;

    let resp = reqwest::Client::new()
        .get(format!(
            "http://{addr}/auth/oauth/github/callback?code=fake-code&state={}",
            "a".repeat(64)
        ))
        .send()
        .await
        .expect("oauth callback");

    assert_eq!(
        resp.status(),
        400,
        "a callback state with nothing stored server-side must be rejected before any \
         token-exchange call fires"
    );
}

#[tokio::test]
async fn oauth_callback_state_length_mismatch_is_rejected_before_equality_comparison() {
    let unreachable_provider = OAuthProviderConfig {
        slug: "github".to_string(),
        client_id: "client-abc".to_string(),
        client_secret: "secret-xyz".to_string(),
        auth_url: "https://github.com/login/oauth/authorize".to_string(),
        token_url: "http://127.0.0.1:1/oauth/token".to_string(),
        userinfo_url: "http://127.0.0.1:1/oauth/userinfo".to_string(),
    };
    let (addr, _tmp) = start_server_with_oauth(vec![unreachable_provider]).await;
    let client = reqwest::Client::builder()
        .cookie_store(true)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("build client");

    // Establish a real stored state via authorize, so this exercises
    // the length check specifically rather than the "nothing stored"
    // branch.
    client
        .get(format!("http://{addr}/auth/oauth/github/authorize"))
        .send()
        .await
        .expect("oauth authorize");

    // 63 hex characters: one short of the 64 the server always mints
    // (32 CSPRNG bytes, hex-encoded).
    let resp = client
        .get(format!(
            "http://{addr}/auth/oauth/github/callback?code=fake-code&state={}",
            "a".repeat(63)
        ))
        .send()
        .await
        .expect("oauth callback");

    assert_eq!(
        resp.status(),
        400,
        "a state of the wrong length must be rejected before any token-exchange call fires"
    );
}

#[tokio::test]
async fn oauth_callback_with_same_length_but_wrong_value_state_is_rejected() {
    let unreachable_provider = OAuthProviderConfig {
        slug: "github".to_string(),
        client_id: "client-abc".to_string(),
        client_secret: "secret-xyz".to_string(),
        auth_url: "https://github.com/login/oauth/authorize".to_string(),
        token_url: "http://127.0.0.1:1/oauth/token".to_string(),
        userinfo_url: "http://127.0.0.1:1/oauth/userinfo".to_string(),
    };
    let (addr, _tmp) = start_server_with_oauth(vec![unreachable_provider]).await;
    let client = reqwest::Client::builder()
        .cookie_store(true)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("build client");

    client
        .get(format!("http://{addr}/auth/oauth/github/authorize"))
        .send()
        .await
        .expect("oauth authorize");

    // 64 hex characters, the correct width, but not the value the
    // server actually stored: proves the equality comparison itself
    // still runs once the length guard passes.
    let resp = client
        .get(format!(
            "http://{addr}/auth/oauth/github/callback?code=fake-code&state={}",
            "a".repeat(64)
        ))
        .send()
        .await
        .expect("oauth callback");

    assert_eq!(
        resp.status(),
        400,
        "a same-length but wrong-value state must still be rejected"
    );
}

// ── OAuth state is single-use ───────────────────────────────────────

#[tokio::test]
async fn oauth_state_is_single_use_a_replayed_valid_callback_is_rejected_the_second_time() {
    let fake_addr = start_fake_oauth_provider().await;
    let provider = OAuthProviderConfig {
        slug: "github".to_string(),
        client_id: "client-abc".to_string(),
        client_secret: "secret-xyz".to_string(),
        auth_url: format!("http://{fake_addr}/authorize"),
        token_url: format!("http://{fake_addr}/token"),
        userinfo_url: format!("http://{fake_addr}/userinfo"),
    };
    let (addr, _tmp) = start_server_with_oauth(vec![provider]).await;
    let client = reqwest::Client::builder()
        .cookie_store(true)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("build client");

    let authorize_resp = client
        .get(format!("http://{addr}/auth/oauth/github/authorize"))
        .send()
        .await
        .expect("oauth authorize");
    let location = authorize_resp
        .headers()
        .get(reqwest::header::LOCATION)
        .expect("Location header")
        .to_str()
        .expect("utf-8")
        .to_string();
    let state = extract_state_param(&location);

    let first = client
        .get(format!(
            "http://{addr}/auth/oauth/github/callback?code=fake-code&state={state}"
        ))
        .send()
        .await
        .expect("first oauth callback");
    assert_eq!(
        first.status(),
        200,
        "the first, genuine callback must complete the login"
    );

    let second = client
        .get(format!(
            "http://{addr}/auth/oauth/github/callback?code=fake-code&state={state}"
        ))
        .send()
        .await
        .expect("second oauth callback");
    assert_eq!(
        second.status(),
        400,
        "replaying the same state a second time must be rejected: the session key was \
         already removed by the first callback"
    );
}

// ── Passkey registration requires an authenticated session ─────────
//
// `passkey_register_start`/`finish` used to accept an arbitrary
// `user_id` straight from the unauthenticated JSON body, which let
// anyone enroll their own authenticator onto any victim account
// (full account takeover). Both handlers now source the identity
// exclusively from the caller's own `AuthSession`, so the request
// body no longer carries any `user_id` field at all.

#[tokio::test]
async fn passkey_register_start_unauthenticated_returns_401() {
    let (addr, _tmp) = start_server_with_oauth(vec![]).await;
    let resp = reqwest::Client::new()
        .post(format!("http://{addr}/auth/passkey/register/start"))
        .send()
        .await
        .expect("passkey register start");
    assert_eq!(resp.status(), 401);
}

#[tokio::test]
async fn passkey_register_finish_unauthenticated_returns_401() {
    let (addr, _tmp) = start_server_with_oauth(vec![]).await;
    let resp = reqwest::Client::new()
        .post(format!("http://{addr}/auth/passkey/register/finish"))
        .json(&json!({
            "credential_name": "laptop",
            "response": {
                "id": "AAAA",
                "rawId": "AAAA",
                "type": "public-key",
                "response": {
                    "attestationObject": "AAAA",
                    "clientDataJSON": "AAAA"
                }
            }
        }))
        .send()
        .await
        .expect("passkey register finish");
    assert_eq!(resp.status(), 401);
}

#[tokio::test]
async fn passkey_register_start_authenticated_succeeds_for_own_account() {
    let (addr, _tmp) = start_server_with_oauth(vec![]).await;
    let client = register_and_login(addr, "alice", "hunter22").await;

    let resp = client
        .post(format!("http://{addr}/auth/passkey/register/start"))
        .send()
        .await
        .expect("passkey register start");
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.expect("json");
    assert!(
        body.get("publicKey").is_some(),
        "expected a publicKey challenge object, got {body}"
    );
}

#[tokio::test]
async fn passkey_register_finish_without_pending_state_returns_400() {
    let (addr, _tmp) = start_server_with_oauth(vec![]).await;
    // An authenticated caller (session identity resolves the pending
    // registration, not a body field) that never called register/start
    // has no entry in the in-memory pending map.
    let client = register_and_login(addr, "alice", "hunter22").await;

    // The `response` field must still be a well-formed JSON object
    // because axum's Json extractor runs before the handler body.
    // We pass a minimal placeholder; the handler should reject on
    // the missing pending state before it tries to finish the
    // webauthn ceremony. If axum rejects earlier, a 4xx still covers
    // the contract we care about (no 5xx, no panic).
    let resp = client
        .post(format!("http://{addr}/auth/passkey/register/finish"))
        .json(&json!({
            "credential_name": "laptop",
            "response": {
                "id": "AAAA",
                "rawId": "AAAA",
                "type": "public-key",
                "response": {
                    "attestationObject": "AAAA",
                    "clientDataJSON": "AAAA"
                }
            }
        }))
        .send()
        .await
        .expect("passkey register finish");
    assert!(
        resp.status().is_client_error(),
        "expected 4xx for missing pending state, got {}",
        resp.status()
    );
}

#[tokio::test]
async fn passkey_login_start_unknown_handle_returns_404() {
    let (addr, _tmp) = start_server_with_oauth(vec![]).await;
    let resp = reqwest::Client::new()
        .post(format!("http://{addr}/auth/passkey/login/start"))
        .json(&json!({ "handle": "ghost" }))
        .send()
        .await
        .expect("passkey login start");
    assert_eq!(resp.status(), 404);
}

#[tokio::test]
async fn passkey_login_start_user_without_credentials_returns_400() {
    let (addr, _tmp) = start_server_with_oauth(vec![]).await;
    // Register a user via password so a user row exists, but skip
    // passkey enrollment; the start handler must return 400.
    register_test_user(addr, "alice", "hunter22").await;

    let resp = reqwest::Client::new()
        .post(format!("http://{addr}/auth/passkey/login/start"))
        .json(&json!({ "handle": "alice" }))
        .send()
        .await
        .expect("passkey login start");
    assert_eq!(resp.status(), 400);
}

// ── Helpers ─────────────────────────────────────────────────────────

/// Create a user through the real `POST /auth/register` endpoint
/// and return the resulting user id as a string. Reuses the same
/// code path the happy-case auth_flow.rs hits, so seeding here also
/// exercises the register route once more on the coverage side.
async fn register_test_user(addr: SocketAddr, handle: &str, password: &str) -> String {
    let resp = reqwest::Client::new()
        .post(format!("http://{addr}/auth/register"))
        .json(&json!({ "handle": handle, "password": password }))
        .send()
        .await
        .expect("register");
    assert_eq!(resp.status(), 201, "register should succeed");
    let body: serde_json::Value = resp.json().await.expect("json");
    body["user_id"]
        .as_str()
        .expect("user_id string")
        .to_string()
}

/// Register then log in a fresh user, returning a `reqwest::Client`
/// with a cookie jar so the `axum-login` session cookie set by
/// `/auth/login` is carried on every subsequent request made with
/// the returned client. Used to exercise routes that require an
/// authenticated `AuthSession`.
async fn register_and_login(addr: SocketAddr, handle: &str, password: &str) -> reqwest::Client {
    let client = reqwest::Client::builder()
        .cookie_store(true)
        .build()
        .expect("build cookie-enabled client");

    let resp = client
        .post(format!("http://{addr}/auth/register"))
        .json(&json!({ "handle": handle, "password": password }))
        .send()
        .await
        .expect("register");
    assert_eq!(resp.status(), 201, "register should succeed");

    let resp = client
        .post(format!("http://{addr}/auth/login"))
        .json(&json!({ "handle": handle, "password": password }))
        .send()
        .await
        .expect("login");
    assert_eq!(resp.status(), 200, "login should succeed");

    client
}
