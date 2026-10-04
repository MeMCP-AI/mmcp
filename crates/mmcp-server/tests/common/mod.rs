//! Test-only `ServerConfig` builder shared across mmcp-server's
//! integration test binaries.
//!
//! Each file under `tests/` compiles as an independent crate, so
//! this module is pulled in via `mod common;` (never a peer `use`)
//! in every file that needs it.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use axum::{
    Json, Router,
    body::Bytes,
    routing::{get, post},
};
use mmcp_server::config::test_support::minimal_server_config;
use mmcp_server::config::{OAuthProviderConfig, ServerConfig};
use serde_json::json;

/// Form fields [`start_fake_oauth_provider`]'s `/token` route observed on
/// the exchange request, so callers can assert on the actual wire request
/// `oauth2` sent rather than only on whether the exchange succeeded.
#[derive(Clone, Default)]
#[allow(dead_code)] // Live in sibling test binaries; each tests/*.rs compiles common as its own crate.
pub struct FakeTokenExchangeProbes {
    /// Set once a non-empty `code_verifier` form field is seen (PKCE).
    pub code_verifier_received: Arc<AtomicBool>,
    /// Set once a non-empty `client_id` form field is seen: proves the
    /// client authenticates via `AuthType::RequestBody` (client_id in
    /// the form body), not the header-based `AuthType::BasicAuth`
    /// `oauth2` defaults to once a client secret is set.
    pub client_id_in_body_received: Arc<AtomicBool>,
}

/// Spins up a minimal fake OAuth provider (token exchange + userinfo) on an
/// ephemeral loopback port, so the callback happy path can be proven end to
/// end without a live GitHub dependency. Returns the provider's address
/// alongside the [`FakeTokenExchangeProbes`] its `/token` route fills in.
#[allow(dead_code)] // Live in sibling test binaries; each tests/*.rs compiles common as its own crate.
pub async fn start_fake_oauth_provider() -> (SocketAddr, FakeTokenExchangeProbes) {
    let probes = FakeTokenExchangeProbes::default();
    let token_route_probes = probes.clone();
    let app = Router::new()
        .route(
            "/token",
            post(move |body: Bytes| {
                let probes = token_route_probes.clone();
                async move {
                    // A hand-rolled check, not a URL-decoding library, is
                    // enough here: PKCE code verifiers and OAuth client ids
                    // are both restricted to characters
                    // `application/x-www-form-urlencoded` never
                    // percent-encodes, so a raw substring search on
                    // `key=value` pairs sees each value exactly as sent.
                    let form_body = String::from_utf8_lossy(&body);
                    let field_present = |field: &str| {
                        form_body
                            .split('&')
                            .any(|pair| pair.strip_prefix(field).is_some_and(|v| !v.is_empty()))
                    };
                    probes
                        .code_verifier_received
                        .store(field_present("code_verifier="), Ordering::SeqCst);
                    probes
                        .client_id_in_body_received
                        .store(field_present("client_id="), Ordering::SeqCst);
                    // `token_type` is mandatory for `oauth2`'s
                    // `StandardTokenResponse` deserialization (RFC 6749
                    // section 5.1); omitting it 500s the exchange instead
                    // of the intended 200/400 the tests below assert on.
                    Json(json!({ "access_token": "fake-access-token", "token_type": "bearer" }))
                }
            }),
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
    (addr, probes)
}

/// The GitHub-slugged provider config pointing every endpoint at the
/// fake provider [`start_fake_oauth_provider`] serves at `fake_addr`.
#[allow(dead_code)] // Live in sibling test binaries; each tests/*.rs compiles common as its own crate.
pub fn fake_oauth_provider_config(fake_addr: SocketAddr) -> OAuthProviderConfig {
    OAuthProviderConfig {
        slug: "github".to_string(),
        client_id: "client-abc".to_string(),
        client_secret: "secret-xyz".to_string(),
        auth_url: format!("http://{fake_addr}/authorize"),
        token_url: format!("http://{fake_addr}/token"),
        userinfo_url: format!("http://{fake_addr}/userinfo"),
    }
}

/// Builder for the `ServerConfig` used across integration test
/// suites.
///
/// Every field starts at [`minimal_server_config`]'s fixed test
/// default: an ephemeral loopback bind, an in-memory SQLite database,
/// no OAuth providers, no push token, and the compiled-in
/// `mmcp_auth` password-length and handle-length defaults. Only
/// fields with a setter method below are settable through this
/// builder; `build()` returns the config with every other field at
/// these defaults.
pub struct TestServerConfigBuilder {
    config: ServerConfig,
}

impl TestServerConfigBuilder {
    /// Starts a builder with test defaults for the given repo root.
    ///
    /// `repo_root` is the one field every test genuinely supplies
    /// itself, since each test owns its own tempdir.
    pub fn new(repo_root: PathBuf) -> Self {
        Self {
            config: minimal_server_config(repo_root),
        }
    }

    /// Overrides the database URL (`ServerConfig::database_url`).
    /// [`minimal_server_config`] defaults to an in-memory SQLite database,
    /// which loses every row when the state drops; a persistence test
    /// points two successive states at one file database here.
    #[allow(dead_code)] // Live in sibling test binaries; each tests/*.rs compiles common as its own crate.
    pub fn database_url(mut self, database_url: impl Into<String>) -> Self {
        self.config.database_url = database_url.into();
        self
    }

    /// Overrides the auth token signing key.
    #[allow(dead_code)] // Live in sibling test binaries; each tests/*.rs compiles common as its own crate.
    pub fn token_key(mut self, token_key: [u8; 32]) -> Self {
        self.config.token_key = token_key;
        self
    }

    /// Overrides the configured OAuth providers.
    #[allow(dead_code)] // Live in sibling test binaries; each tests/*.rs compiles common as its own crate.
    pub fn oauth_providers(mut self, oauth_providers: Vec<OAuthProviderConfig>) -> Self {
        self.config.oauth_providers = oauth_providers;
        self
    }

    /// Overrides the maximum accepted account handle length.
    #[allow(dead_code)] // Live in sibling test binaries; each tests/*.rs compiles common as its own crate.
    pub fn max_handle_length(mut self, max_handle_length: usize) -> Self {
        self.config.max_handle_length = max_handle_length;
        self
    }

    /// Overrides whether `POST /auth/register` accepts
    /// self-registration requests. [`minimal_server_config`] defaults
    /// this to `true` (test convenience); pass `false` here for a
    /// test that specifically covers the closed-by-default gate.
    #[allow(dead_code)] // Live in sibling test binaries; each tests/*.rs compiles common as its own crate.
    pub fn allow_self_registration(mut self, allow_self_registration: bool) -> Self {
        self.config.allow_self_registration = allow_self_registration;
        self
    }

    /// Overrides the shared push-token credential
    /// (`ServerConfig::push_token`, env `MMCP_PUSH_TOKEN`).
    /// [`minimal_server_config`] defaults this to `None` (push
    /// disabled); a test covering `POST /sync/push` or
    /// `git-receive-pack`'s write-enforcement path sets a real value
    /// here.
    #[allow(dead_code)] // Live in sibling test binaries; each tests/*.rs compiles common as its own crate.
    pub fn push_token(mut self, push_token: impl Into<String>) -> Self {
        self.config.push_token = Some(push_token.into());
        self
    }

    /// Finishes the builder, returning the built `ServerConfig`.
    pub fn build(self) -> ServerConfig {
        self.config
    }
}
