#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Sessions, passkey ceremonies and OAuth flows survive a server restart on a persistent database.

use std::net::SocketAddr;
use std::path::Path;

use mmcp_db::entities::http_session;
use mmcp_db::repository::user_repo;
use mmcp_server::config::{OAuthProviderConfig, ServerConfig};
use mmcp_server::state::ServerState;
use sea_orm::EntityTrait;
use serde_json::json;
use tempfile::TempDir;
use tokio::task::JoinHandle;

mod common;

/// Token key shared by every server instance of one test, so signed values outlive a restart.
const FIXED_TOKEN_KEY: [u8; 32] = [11u8; 32];

/// Builds a `sqlite://` URL for `db_path`, creating the file when absent, with forward slashes for Windows.
fn sqlite_file_url(db_path: &Path) -> String {
    format!(
        "sqlite://{}?mode=rwc",
        db_path.to_string_lossy().replace('\\', "/")
    )
}

fn config_for(tmp: &TempDir) -> ServerConfig {
    config_with_oauth_providers(tmp, vec![])
}

fn config_with_oauth_providers(
    tmp: &TempDir,
    oauth_providers: Vec<OAuthProviderConfig>,
) -> ServerConfig {
    common::TestServerConfigBuilder::new(tmp.path().to_path_buf())
        .database_url(sqlite_file_url(&tmp.path().join("mmcp.db")))
        .token_key(FIXED_TOKEN_KEY)
        .oauth_providers(oauth_providers)
        .build()
}

/// A running server instance: its address and the task serving it.
struct RunningServer {
    addr: SocketAddr,
    task: JoinHandle<()>,
}

impl RunningServer {
    /// Stops serving and drops the router together with its state.
    /// The database file stays, so a later instance can reopen it.
    async fn stop(self) {
        self.task.abort();
        let _ = self.task.await;
    }
}

async fn start_server(cfg: &ServerConfig) -> RunningServer {
    let state = ServerState::initialize(cfg).await.expect("state init");
    let app = mmcp_server::app::build_router(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("local addr");
    let task = tokio::spawn(async move {
        axum::serve(listener, app.into_make_service())
            .await
            .unwrap();
    });
    RunningServer { addr, task }
}

fn cookie_client() -> reqwest::Client {
    reqwest::Client::builder()
        .cookie_store(true)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("build client")
}

async fn register_and_login(client: &reqwest::Client, addr: SocketAddr, handle: &str) {
    let credentials = json!({ "handle": handle, "password": "hunter22hunter22" });
    let register = client
        .post(format!("http://{addr}/auth/register"))
        .json(&credentials)
        .send()
        .await
        .expect("register");
    assert_eq!(register.status(), 201);
    let login = client
        .post(format!("http://{addr}/auth/login"))
        .json(&credentials)
        .send()
        .await
        .expect("login");
    assert_eq!(login.status(), 200);
}

/// Status of `POST /auth/passkey/register/start`, a route that requires an authenticated session.
async fn passkey_register_start_status(
    client: &reqwest::Client,
    addr: SocketAddr,
) -> reqwest::StatusCode {
    client
        .post(format!("http://{addr}/auth/passkey/register/start"))
        .send()
        .await
        .expect("passkey register start")
        .status()
}

/// Rows currently in `http_sessions`, read through the test's own connection to the database file.
async fn http_session_rows(db_url: &str) -> Vec<http_session::Model> {
    let db = mmcp_db::connect(db_url).await.expect("connect to the file");
    http_session::Entity::find()
        .all(db.connection())
        .await
        .expect("read http_sessions")
}

/// The whole `Set-Cookie` header of the cookie named `name` that `response` sets, if any.
fn set_cookie_header(response: &reqwest::Response, name: &str) -> Option<String> {
    response
        .headers()
        .get_all(reqwest::header::SET_COOKIE)
        .iter()
        .filter_map(|header| header.to_str().ok())
        .find(|header| header.starts_with(&format!("{name}=")))
        .map(str::to_owned)
}

/// Value of the cookie named `name` set by `response`, if any.
fn set_cookie_value(response: &reqwest::Response, name: &str) -> Option<String> {
    set_cookie_header(response, name).and_then(|header| {
        header
            .split(';')
            .next()
            .and_then(|pair| pair.strip_prefix(&format!("{name}=")))
            .map(str::to_owned)
    })
}

#[tokio::test]
async fn session_rows_hold_neither_the_cookie_id_nor_the_password_hash() {
    let tmp = TempDir::new().expect("tempdir");
    let cfg = config_for(&tmp);
    let server = start_server(&cfg).await;
    let client = reqwest::Client::new();
    let credentials = json!({ "handle": "alice", "password": "hunter22hunter22" });
    client
        .post(format!("http://{}/auth/register", server.addr))
        .json(&credentials)
        .send()
        .await
        .expect("register");

    let login = client
        .post(format!("http://{}/auth/login", server.addr))
        .json(&credentials)
        .send()
        .await
        .expect("login");
    assert_eq!(login.status(), 200);
    let cookie_id = set_cookie_value(&login, "id").expect("login sets the session cookie");

    let rows = http_session_rows(&cfg.database_url).await;
    assert_eq!(rows.len(), 1, "one login stores one session row");
    assert_ne!(
        rows[0].session_id_sha256, cookie_id,
        "the cookie value must never be a row key"
    );
    let db = mmcp_db::connect(&cfg.database_url).await.expect("connect");
    let user = user_repo::find_by_handle(db.connection(), "alice")
        .await
        .expect("find user")
        .expect("registered user exists");
    let password_hash = user.password_hash.expect("a password user has a hash");
    assert!(
        !rows[0].data.contains(&password_hash),
        "the password hash must not be stored in the session data"
    );
    server.stop().await;
}

#[tokio::test]
async fn oauth_authorize_writes_no_session_row() {
    const AUTHORIZE_CALLS: usize = 3;
    let tmp = TempDir::new().expect("tempdir");
    let cfg = config_with_oauth_providers(
        &tmp,
        vec![OAuthProviderConfig::github("client-abc", "secret-xyz")],
    );
    let server = start_server(&cfg).await;
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("build client");

    for _ in 0..AUTHORIZE_CALLS {
        let response = client
            .get(format!(
                "http://{}/auth/oauth/github/authorize",
                server.addr
            ))
            .send()
            .await
            .expect("oauth authorize");
        assert!(response.status().is_redirection());
    }

    assert!(
        http_session_rows(&cfg.database_url).await.is_empty(),
        "an anonymous authorize must not write a session row"
    );
    server.stop().await;
}

/// Name of the cookie carrying a pending OAuth flow against the `github` provider on an HTTP origin.
const OAUTH_FLOW_COOKIE_NAME: &str = "mmcp_oauth_flow_github";

/// Lifetime of a pending OAuth flow and of a session between requests: fifteen minutes.
const FIFTEEN_MINUTES_SECS: u64 = 15 * 60;

/// An authorize call's `state` query parameter and flow cookie value.
struct StartedOauthFlow {
    state: String,
    flow_cookie_value: String,
}

/// Starts an OAuth flow against the `github` provider and returns what a browser would carry to the callback.
async fn start_oauth_flow(client: &reqwest::Client, addr: SocketAddr) -> StartedOauthFlow {
    let authorize = client
        .get(format!("http://{addr}/auth/oauth/github/authorize"))
        .send()
        .await
        .expect("oauth authorize");
    assert!(authorize.status().is_redirection());
    let location = authorize
        .headers()
        .get(reqwest::header::LOCATION)
        .expect("Location header")
        .to_str()
        .expect("utf-8")
        .to_owned();
    let state = location
        .split("state=")
        .nth(1)
        .expect("redirect Location must carry a state= query parameter")
        .split('&')
        .next()
        .unwrap_or_default()
        .to_owned();
    let flow_cookie_value = set_cookie_value(&authorize, OAUTH_FLOW_COOKIE_NAME)
        .expect("authorize sets the flow cookie");
    StartedOauthFlow {
        state,
        flow_cookie_value,
    }
}

/// Callback request carrying an explicit flow cookie value, so no cookie jar decides what is sent.
async fn oauth_callback_with_cookie(
    client: &reqwest::Client,
    addr: SocketAddr,
    state: &str,
    flow_cookie_value: &str,
) -> reqwest::Response {
    client
        .get(format!(
            "http://{addr}/auth/oauth/github/callback?code=fake-code&state={state}"
        ))
        .header(
            reqwest::header::COOKIE,
            format!("{OAUTH_FLOW_COOKIE_NAME}={flow_cookie_value}"),
        )
        .send()
        .await
        .expect("oauth callback")
}

fn no_redirect_client() -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("build client")
}

#[tokio::test]
async fn oauth_flow_cookie_is_httponly_lax_and_cleared_at_callback() {
    let tmp = TempDir::new().expect("tempdir");
    let (fake_addr, _probes) = common::start_fake_oauth_provider().await;
    let cfg =
        config_with_oauth_providers(&tmp, vec![common::fake_oauth_provider_config(fake_addr)]);
    let server = start_server(&cfg).await;
    let client = no_redirect_client();

    let authorize = client
        .get(format!(
            "http://{}/auth/oauth/github/authorize",
            server.addr
        ))
        .send()
        .await
        .expect("oauth authorize");
    let flow_cookie = set_cookie_header(&authorize, OAUTH_FLOW_COOKIE_NAME)
        .expect("authorize sets the flow cookie")
        .to_lowercase();
    assert!(flow_cookie.contains("httponly"), "{flow_cookie}");
    assert!(flow_cookie.contains("samesite=lax"), "{flow_cookie}");
    assert!(flow_cookie.contains("path=/"), "{flow_cookie}");
    assert!(
        flow_cookie.contains(&format!("max-age={FIFTEEN_MINUTES_SECS}")),
        "{flow_cookie}"
    );
    assert!(
        !flow_cookie.contains("secure"),
        "an HTTP origin must not set Secure: {flow_cookie}"
    );

    let started = start_oauth_flow(&client, server.addr).await;
    let callback = oauth_callback_with_cookie(
        &client,
        server.addr,
        &started.state,
        &started.flow_cookie_value,
    )
    .await;
    assert_eq!(callback.status(), 200);
    let cleared = set_cookie_header(&callback, OAUTH_FLOW_COOKIE_NAME)
        .expect("the callback response clears the flow cookie")
        .to_lowercase();
    assert!(cleared.contains("max-age=0"), "{cleared}");
    server.stop().await;
}

#[tokio::test]
async fn oauth_callback_with_expired_or_tampered_flow_cookie_is_rejected_before_token_exchange() {
    let tmp = TempDir::new().expect("tempdir");
    // Nothing listens on the provider's token endpoint: a live exchange would
    // fail as a 500, so a 400 proves the exchange was never attempted.
    let unreachable_provider = OAuthProviderConfig {
        slug: "github".to_string(),
        client_id: "client-abc".to_string(),
        client_secret: "secret-xyz".to_string(),
        auth_url: "https://github.com/login/oauth/authorize".to_string(),
        token_url: "http://127.0.0.1:1/oauth/token".to_string(),
        userinfo_url: "http://127.0.0.1:1/oauth/userinfo".to_string(),
    };
    let cfg = config_with_oauth_providers(&tmp, vec![unreachable_provider]);
    let server = start_server(&cfg).await;
    let client = no_redirect_client();
    let started = start_oauth_flow(&client, server.addr).await;

    let mut tampered = started.flow_cookie_value.clone().into_bytes();
    let last = tampered.len() - 1;
    tampered[last] = if tampered[last] == b'A' { b'B' } else { b'A' };
    let tampered = String::from_utf8(tampered).unwrap();
    let rejected_tampered =
        oauth_callback_with_cookie(&client, server.addr, &started.state, &tampered).await;
    assert_eq!(rejected_tampered.status(), 400);
    assert!(set_cookie_header(&rejected_tampered, OAUTH_FLOW_COOKIE_NAME).is_some());

    let expired = mmcp_auth::OauthFlowTokenCodec::from_key(&FIXED_TOKEN_KEY)
        .seal(&mmcp_auth::OauthFlowClaims {
            provider: "github".to_owned(),
            csrf_state: started.state.clone(),
            pkce_verifier: "verifier".to_owned(),
            expires_at: jiff::Timestamp::now().as_second() - 1,
        })
        .expect("seal an already expired flow");
    let rejected_expired =
        oauth_callback_with_cookie(&client, server.addr, &started.state, &expired).await;
    assert_eq!(rejected_expired.status(), 400);
    assert!(set_cookie_header(&rejected_expired, OAUTH_FLOW_COOKIE_NAME).is_some());
    server.stop().await;
}

#[tokio::test]
async fn oauth_callback_succeeds_across_restart_with_a_fixed_token_key() {
    let tmp = TempDir::new().expect("tempdir");
    let (fake_addr, _probes) = common::start_fake_oauth_provider().await;
    let cfg =
        config_with_oauth_providers(&tmp, vec![common::fake_oauth_provider_config(fake_addr)]);
    let client = no_redirect_client();

    let first = start_server(&cfg).await;
    let started = start_oauth_flow(&client, first.addr).await;
    first.stop().await;

    let second = start_server(&cfg).await;
    let callback = oauth_callback_with_cookie(
        &client,
        second.addr,
        &started.state,
        &started.flow_cookie_value,
    )
    .await;
    let status = callback.status();
    let body = callback.text().await.unwrap_or_default();
    assert_eq!(
        status, 200,
        "a flow started before the restart must complete after it, got body: {body}"
    );
    assert!(body.contains("OAuth login successful"));
    second.stop().await;
}

/// Passkey login cannot complete without an authenticator in the test graph;
/// it shares the layer configuration these two logins exercise.
#[tokio::test]
async fn session_cookie_max_age_after_login_is_the_inactivity_window() {
    let tmp = TempDir::new().expect("tempdir");
    let (fake_addr, _probes) = common::start_fake_oauth_provider().await;
    let cfg =
        config_with_oauth_providers(&tmp, vec![common::fake_oauth_provider_config(fake_addr)]);
    let server = start_server(&cfg).await;
    let client = no_redirect_client();
    let expected_max_age = format!("max-age={FIFTEEN_MINUTES_SECS}");

    let credentials = json!({ "handle": "alice", "password": "hunter22hunter22" });
    client
        .post(format!("http://{}/auth/register", server.addr))
        .json(&credentials)
        .send()
        .await
        .expect("register");
    let password_login = client
        .post(format!("http://{}/auth/login", server.addr))
        .json(&credentials)
        .send()
        .await
        .expect("password login");
    let password_cookie = set_cookie_header(&password_login, "id")
        .expect("a password login sets the session cookie")
        .to_lowercase();
    assert!(
        password_cookie.contains(&expected_max_age),
        "{password_cookie}"
    );

    let started = start_oauth_flow(&client, server.addr).await;
    let oauth_login = oauth_callback_with_cookie(
        &client,
        server.addr,
        &started.state,
        &started.flow_cookie_value,
    )
    .await;
    assert_eq!(oauth_login.status(), 200);
    let oauth_cookie = set_cookie_header(&oauth_login, "id")
        .expect("an OAuth login sets the session cookie")
        .to_lowercase();
    assert!(oauth_cookie.contains(&expected_max_age), "{oauth_cookie}");
    server.stop().await;
}

#[tokio::test]
async fn session_survives_server_restart_on_a_persistent_database() {
    let tmp = TempDir::new().expect("tempdir");
    let cfg = config_for(&tmp);
    let client = cookie_client();

    let first = start_server(&cfg).await;
    register_and_login(&client, first.addr, "alice").await;
    assert_eq!(
        passkey_register_start_status(&client, first.addr).await,
        200,
        "the logged-in session must authenticate before the restart"
    );
    first.stop().await;

    let second = start_server(&cfg).await;
    assert_eq!(
        passkey_register_start_status(&client, second.addr).await,
        200,
        "the same session cookie must still authenticate after the restart"
    );
    second.stop().await;
}
