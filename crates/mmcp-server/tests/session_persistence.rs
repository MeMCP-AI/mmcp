#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Sessions, passkey ceremonies and OAuth flows survive a server restart on a persistent database.

use std::net::SocketAddr;
use std::path::Path;

use mmcp_db::entities::http_session;
use mmcp_db::repository::{http_session_repo, passkey_repo, user_repo};
use mmcp_server::config::{OAuthProviderConfig, ServerConfig};
use mmcp_server::state::ServerState;
use sea_orm::EntityTrait;
use serde_json::json;
use tempfile::TempDir;
use tokio::task::JoinHandle;

mod common;

/// Key under which `axum-login` stores the user id and auth hash in the session data.
const AXUM_LOGIN_DATA_KEY: &str = "axum-login.data";

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
    assert_stored_auth_hash_is_the_digest(&rows[0].data, &password_hash, user.credential_epoch);
    server.stop().await;
}

/// The session data holds the digest of the credential state as its auth hash, never the password hash.
///
/// `axum-login` stores the auth hash as a JSON array of byte values, so a raw password hash is not found as text:
/// the check compares the stored bytes.
fn assert_stored_auth_hash_is_the_digest(
    session_data: &str,
    password_hash: &str,
    credential_epoch: i64,
) {
    assert!(
        !session_data.contains(password_hash),
        "the password hash must not be stored in the session data as text"
    );
    let data: serde_json::Value = serde_json::from_str(session_data).expect("session data json");
    let stored_auth_hash: Vec<u8> = data[AXUM_LOGIN_DATA_KEY]["auth_hash"]
        .as_array()
        .expect("the session stores the auth hash as an array")
        .iter()
        .map(|byte| u8::try_from(byte.as_u64().expect("a byte value")).expect("a byte"))
        .collect();
    assert_ne!(
        stored_auth_hash,
        password_hash.as_bytes(),
        "the password hash bytes must not be the stored auth hash"
    );
    assert_eq!(
        stored_auth_hash,
        mmcp_auth::session_auth_hash::session_auth_hash(credential_epoch, Some(password_hash)),
        "the stored auth hash is the digest of the credential state"
    );
}

/// Session data whose auth hash is the raw password hash bytes, the defect the digest check guards.
#[test]
#[should_panic(expected = "the password hash bytes must not be the stored auth hash")]
fn the_auth_hash_check_rejects_raw_password_hash_bytes() {
    let password_hash = "$argon2id$v=19$m=19456,t=2,p=1$c2FsdHNhbHQ$aGFzaGhhc2g";
    let session_data = json!({
        (AXUM_LOGIN_DATA_KEY): { "auth_hash": password_hash.as_bytes() }
    })
    .to_string();
    assert!(
        !session_data.contains(password_hash),
        "the earlier text check passes on this data, which is why it was vacuous"
    );

    assert_stored_auth_hash_is_the_digest(&session_data, password_hash, 0);
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

/// Session data key the passkey authentication ceremony is stored under.
/// Mirrors the server's own key: the test reads the stored ceremony back.
const PASSKEY_AUTHENTICATION_SESSION_KEY: &str = "passkey_authentication_ceremony";

/// Body of a registration finish whose response is well-formed JSON but carries no real attestation.
fn unsigned_registration_finish_body() -> serde_json::Value {
    json!({
        "credential_name": "laptop",
        "response": {
            "id": "AAAA",
            "rawId": "AAAA",
            "type": "public-key",
            "response": { "attestationObject": "AAAA", "clientDataJSON": "AAAA" }
        }
    })
}

/// Body the server answers a registration finish with when the session holds no ceremony for the caller.
const NO_PENDING_REGISTRATION_BODY: &str = "no pending registration for this user";

/// A stored passkey credential of an authenticator no test controls: enough for a login to start, never to finish.
fn enrolled_passkey_json() -> serde_json::Value {
    const ZERO_COORDINATE_BASE64URL: &str = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
    let passkey = json!({
        "cred": {
            "cred_id": "AAAAAAAAAAAAAAAAAAAAAA",
            "cred": {
                "type_": "ES256",
                "key": { "EC_EC2": {
                    "curve": "SECP256R1",
                    "x": ZERO_COORDINATE_BASE64URL,
                    "y": ZERO_COORDINATE_BASE64URL
                } }
            },
            "counter": 0,
            "transports": null,
            "user_verified": false,
            "backup_eligible": false,
            "backup_state": false,
            "registration_policy": "preferred",
            "extensions": {},
            "attestation": { "data": "None", "metadata": "None" },
            "attestation_format": "none"
        }
    });
    serde_json::from_value::<webauthn_rs::prelude::Passkey>(passkey.clone())
        .expect("the passkey fixture must deserialize as a real Passkey");
    passkey
}

/// Registers `handle` through the API and enrolls the fixture passkey on the account.
async fn register_user_with_passkey(
    cfg: &ServerConfig,
    addr: SocketAddr,
    handle: &str,
) -> uuid::Uuid {
    let register = reqwest::Client::new()
        .post(format!("http://{addr}/auth/register"))
        .json(&json!({ "handle": handle, "password": "hunter22hunter22" }))
        .send()
        .await
        .expect("register");
    assert_eq!(register.status(), 201);
    let body: serde_json::Value = register.json().await.expect("register body");
    let user_id: uuid::Uuid = body["user_id"]
        .as_str()
        .expect("user_id string")
        .parse()
        .expect("user_id uuid");

    let db = mmcp_db::connect(&cfg.database_url).await.expect("connect");
    passkey_repo::create(
        db.connection(),
        uuid::Uuid::now_v7(),
        user_id,
        "fixture".to_owned(),
        enrolled_passkey_json().to_string(),
        0,
        None,
    )
    .await
    .expect("enroll the fixture passkey");
    user_id
}

async fn passkey_login_start(client: &reqwest::Client, addr: SocketAddr, handle: &str) {
    let response = client
        .post(format!("http://{addr}/auth/passkey/login/start"))
        .json(&json!({ "handle": handle }))
        .send()
        .await
        .expect("passkey login start");
    assert_eq!(
        response.status(),
        200,
        "a handle with an enrolled passkey must start a login ceremony"
    );
}

async fn passkey_credential_count(cfg: &ServerConfig, user_id: uuid::Uuid) -> usize {
    let db = mmcp_db::connect(&cfg.database_url).await.expect("connect");
    passkey_repo::find_by_user(db.connection(), user_id)
        .await
        .expect("read credentials")
        .len()
}

#[tokio::test]
async fn passkey_registration_ceremony_survives_server_restart() {
    let tmp = TempDir::new().expect("tempdir");
    let cfg = config_for(&tmp);
    let client = cookie_client();

    let first = start_server(&cfg).await;
    register_and_login(&client, first.addr, "alice").await;
    assert_eq!(
        passkey_register_start_status(&client, first.addr).await,
        200
    );
    first.stop().await;

    let second = start_server(&cfg).await;
    let finish = client
        .post(format!(
            "http://{}/auth/passkey/register/finish",
            second.addr
        ))
        .json(&unsigned_registration_finish_body())
        .send()
        .await
        .expect("passkey register finish");
    let status = finish.status();
    let body = finish.text().await.unwrap_or_default();
    assert_ne!(
        status,
        reqwest::StatusCode::UNAUTHORIZED,
        "the session itself must survive the restart"
    );
    assert_ne!(
        body, NO_PENDING_REGISTRATION_BODY,
        "the ceremony started before the restart must reach webauthn verification after it"
    );
    second.stop().await;
}

#[tokio::test]
async fn passkey_registration_ceremony_is_refused_for_a_different_session_user() {
    let tmp = TempDir::new().expect("tempdir");
    let cfg = config_for(&tmp);
    let server = start_server(&cfg).await;
    let client = cookie_client();

    register_and_login(&client, server.addr, "alice").await;
    assert_eq!(
        passkey_register_start_status(&client, server.addr).await,
        200,
        "alice starts a registration ceremony"
    );
    // The same browser then logs in as bob: a login keeps the session data.
    register_and_login(&client, server.addr, "bob").await;

    let finish = client
        .post(format!(
            "http://{}/auth/passkey/register/finish",
            server.addr
        ))
        .json(&unsigned_registration_finish_body())
        .send()
        .await
        .expect("passkey register finish");
    assert_eq!(finish.status(), 400);
    assert_eq!(
        finish.text().await.unwrap_or_default(),
        NO_PENDING_REGISTRATION_BODY,
        "bob must be refused alice's ceremony before any webauthn call"
    );

    let db = mmcp_db::connect(&cfg.database_url).await.expect("connect");
    for handle in ["alice", "bob"] {
        let user = user_repo::find_by_handle(db.connection(), handle)
            .await
            .expect("find user")
            .expect("user exists");
        assert_eq!(
            passkey_credential_count(&cfg, user.id).await,
            0,
            "no credential row may be written for {handle}"
        );
    }
    server.stop().await;
}

async fn registration_finish_body(client: &reqwest::Client, addr: SocketAddr) -> String {
    client
        .post(format!("http://{addr}/auth/passkey/register/finish"))
        .json(&unsigned_registration_finish_body())
        .send()
        .await
        .expect("passkey register finish")
        .text()
        .await
        .unwrap_or_default()
}

#[tokio::test]
async fn passkey_registration_ceremony_is_consumed_when_the_finish_fails() {
    let tmp = TempDir::new().expect("tempdir");
    let cfg = config_for(&tmp);
    let server = start_server(&cfg).await;
    let client = cookie_client();
    register_and_login(&client, server.addr, "alice").await;
    assert_eq!(
        passkey_register_start_status(&client, server.addr).await,
        200
    );

    let first = registration_finish_body(&client, server.addr).await;
    let second = registration_finish_body(&client, server.addr).await;

    assert_ne!(
        first, NO_PENDING_REGISTRATION_BODY,
        "the first finish reaches webauthn verification and fails there"
    );
    assert_eq!(
        second, NO_PENDING_REGISTRATION_BODY,
        "a failed finish must still consume the ceremony"
    );
    server.stop().await;
}

#[tokio::test]
async fn concurrent_passkey_registration_finishes_on_one_cookie_reach_verification_once() {
    const FINISHES: usize = 8;
    let tmp = TempDir::new().expect("tempdir");
    let cfg = config_for(&tmp);
    let server = start_server(&cfg).await;
    let client = cookie_client();
    register_and_login(&client, server.addr, "alice").await;
    assert_eq!(
        passkey_register_start_status(&client, server.addr).await,
        200
    );

    let finishes: Vec<_> = (0..FINISHES)
        .map(|_| {
            let client = client.clone();
            let addr = server.addr;
            tokio::spawn(async move { registration_finish_body(&client, addr).await })
        })
        .collect();
    let mut reached_verification = 0;
    for finish in finishes {
        if finish.await.expect("finish task") != NO_PENDING_REGISTRATION_BODY {
            reached_verification += 1;
        }
    }

    assert_eq!(
        reached_verification, 1,
        "one ceremony may be verified once, however many finishes race on its cookie"
    );
    server.stop().await;
}

/// Body of a login finish whose response is well-formed JSON but carries no real assertion.
fn unsigned_login_finish_body(handle: &str) -> serde_json::Value {
    json!({
        "handle": handle,
        "response": {
            "id": "AAAA",
            "rawId": "AAAA",
            "type": "public-key",
            "response": {
                "authenticatorData": "AAAA",
                "clientDataJSON": "AAAA",
                "signature": "AAAA"
            }
        }
    })
}

async fn passkey_login_finish_status(
    client: &reqwest::Client,
    addr: SocketAddr,
    handle: &str,
) -> reqwest::StatusCode {
    client
        .post(format!("http://{addr}/auth/passkey/login/finish"))
        .json(&unsigned_login_finish_body(handle))
        .send()
        .await
        .expect("passkey login finish")
        .status()
}

#[tokio::test]
async fn passkey_login_ceremony_is_consumed_when_the_finish_fails() {
    let tmp = TempDir::new().expect("tempdir");
    let cfg = config_for(&tmp);
    let server = start_server(&cfg).await;
    register_user_with_passkey(&cfg, server.addr, "alice").await;
    let client = cookie_client();
    passkey_login_start(&client, server.addr, "alice").await;
    assert!(
        http_session_rows(&cfg.database_url)
            .await
            .iter()
            .any(|row| row.data.contains(PASSKEY_AUTHENTICATION_SESSION_KEY)),
        "the login start stores its ceremony"
    );

    assert_eq!(
        passkey_login_finish_status(&client, server.addr, "alice").await,
        401,
        "an assertion no authenticator signed is refused"
    );

    assert!(
        http_session_rows(&cfg.database_url)
            .await
            .iter()
            .all(|row| !row.data.contains(PASSKEY_AUTHENTICATION_SESSION_KEY)),
        "a failed finish must consume the ceremony"
    );
    server.stop().await;
}

#[tokio::test]
async fn anonymous_passkey_login_finish_writes_no_session_row() {
    let tmp = TempDir::new().expect("tempdir");
    let cfg = config_for(&tmp);
    let server = start_server(&cfg).await;
    register_user_with_passkey(&cfg, server.addr, "alice").await;

    assert_eq!(
        passkey_login_finish_status(&reqwest::Client::new(), server.addr, "alice").await,
        401
    );

    assert!(
        http_session_rows(&cfg.database_url).await.is_empty(),
        "a finish without a session must not create one"
    );
    server.stop().await;
}

#[tokio::test]
async fn concurrent_passkey_login_starts_for_one_handle_keep_both_ceremonies() {
    let tmp = TempDir::new().expect("tempdir");
    let cfg = config_for(&tmp);
    let server = start_server(&cfg).await;
    let alice = register_user_with_passkey(&cfg, server.addr, "alice").await;

    passkey_login_start(&cookie_client(), server.addr, "alice").await;
    passkey_login_start(&cookie_client(), server.addr, "alice").await;

    let rows = http_session_rows(&cfg.database_url).await;
    assert_eq!(
        rows.len(),
        2,
        "each caller's session holds its own ceremony"
    );
    for row in &rows {
        let data: serde_json::Value = serde_json::from_str(&row.data).expect("session data json");
        let bound_user = data[PASSKEY_AUTHENTICATION_SESSION_KEY]["user_id"]
            .as_str()
            .expect("the session holds an authentication ceremony bound to a user");
        assert_eq!(bound_user, alice.to_string());
    }
    server.stop().await;
}

#[tokio::test]
async fn passkey_login_start_rows_are_removed_by_the_sweep_after_expiry() {
    const LOGIN_STARTS: usize = 3;
    let tmp = TempDir::new().expect("tempdir");
    let cfg = config_for(&tmp);
    let server = start_server(&cfg).await;
    register_user_with_passkey(&cfg, server.addr, "alice").await;
    for _ in 0..LOGIN_STARTS {
        passkey_login_start(&cookie_client(), server.addr, "alice").await;
    }
    let rows = http_session_rows(&cfg.database_url).await;
    assert!(
        rows.len() <= LOGIN_STARTS && !rows.is_empty(),
        "N anonymous starts create at most N rows, got {}",
        rows.len()
    );

    // Expire every row through the repository, then spawn the production sweeper over the same database.
    let db = mmcp_db::connect(&cfg.database_url).await.expect("connect");
    for row in rows {
        http_session_repo::upsert(
            db.connection(),
            http_session::Model {
                expires_at: 0,
                ..row
            },
        )
        .await
        .expect("expire the row");
    }
    let sweeper = mmcp_server::session_store::spawn_expired_session_sweeper(
        mmcp_server::session_store::DatabaseSessionStore::new(db.connection().clone()),
    );
    tokio::time::timeout(SWEEP_WAIT_LIMIT, async {
        while !http_session_rows(&cfg.database_url).await.is_empty() {
            tokio::time::sleep(SWEEP_POLL_INTERVAL).await;
        }
    })
    .await
    .expect("the sweep must delete every expired row");
    sweeper.abort();
    server.stop().await;
}

/// Upper bound on the wait for a sweep, never a pause an assertion depends on.
const SWEEP_WAIT_LIMIT: std::time::Duration = std::time::Duration::from_secs(10);

/// Pause between two looks at the table while waiting for the sweep.
const SWEEP_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(50);

/// Logs `handle` in on a first server, restarts it on the same database, and reuses the cookie on the second.
async fn assert_session_survives_restart(cfg: &ServerConfig, handle: &str) {
    let client = cookie_client();

    let first = start_server(cfg).await;
    register_and_login(&client, first.addr, handle).await;
    assert_eq!(
        passkey_register_start_status(&client, first.addr).await,
        200,
        "the logged-in session must authenticate before the restart"
    );
    first.stop().await;

    let second = start_server(cfg).await;
    assert_eq!(
        passkey_register_start_status(&client, second.addr).await,
        200,
        "the same session cookie must still authenticate after the restart"
    );
    second.stop().await;
}

#[tokio::test]
async fn session_survives_server_restart_on_a_persistent_database() {
    let tmp = TempDir::new().expect("tempdir");
    assert_session_survives_restart(&config_for(&tmp), "alice").await;
}

/// Environment variable naming the Postgres database the ignored Postgres test runs against.
const POSTGRES_TEST_URL_ENV: &str = "POSTGRES_TEST_URL";

/// Restart persistence and the store primitives on a real Postgres server.
///
/// Run with `--ignored` and `POSTGRES_TEST_URL` set to a disposable database.
/// Handles are unique per run, so one database serves repeated runs.
#[tokio::test]
#[ignore = "needs a disposable Postgres database named by POSTGRES_TEST_URL"]
async fn session_survives_server_restart_on_postgres() {
    let database_url = std::env::var(POSTGRES_TEST_URL_ENV).unwrap_or_else(|_| {
        panic!("{POSTGRES_TEST_URL_ENV} must name a disposable Postgres database")
    });
    let tmp = TempDir::new().expect("tempdir");
    let cfg = common::TestServerConfigBuilder::new(tmp.path().to_path_buf())
        .database_url(database_url)
        .token_key(FIXED_TOKEN_KEY)
        .build();
    let handle = format!("alice-{}", uuid::Uuid::now_v7().simple());

    assert_session_survives_restart(&cfg, &handle).await;

    assert_store_primitives_work_on(&cfg.database_url).await;
}

/// Exercises the conflict, upsert and batched-delete statements the store issues, on the database at `database_url`.
async fn assert_store_primitives_work_on(database_url: &str) {
    use time::{Duration, OffsetDateTime};
    use tower_sessions::SessionStore;
    use tower_sessions::session::{Id, Record};

    const EXPIRED_RECORDS: usize = 5;
    const BATCH_SIZE: u64 = 2;
    let db = mmcp_db::connect(database_url).await.expect("connect");
    let store = mmcp_server::session_store::DatabaseSessionStore::new(db.connection().clone());
    let record_expiring_in = |lifetime: Duration| Record {
        id: Id::default(),
        data: std::collections::HashMap::new(),
        expiry_date: OffsetDateTime::now_utc() + lifetime,
    };

    let mut first = record_expiring_in(Duration::minutes(15));
    store.create(&mut first).await.expect("create a record");
    let mut colliding = record_expiring_in(Duration::minutes(15));
    colliding.id = first.id;
    store
        .create(&mut colliding)
        .await
        .expect("a colliding create draws a fresh id");
    assert_ne!(
        colliding.id, first.id,
        "a taken id is replaced by a fresh one"
    );
    first
        .data
        .insert("saved".to_owned(), serde_json::json!(true));
    store
        .save(&first)
        .await
        .expect("an upsert on an existing key");
    let reloaded = store.load(&first.id).await.expect("load").expect("present");
    assert_eq!(reloaded.data, first.data);

    let mut expired_ids = Vec::new();
    for _ in 0..EXPIRED_RECORDS {
        let mut expired = record_expiring_in(Duration::minutes(-1));
        store.create(&mut expired).await.expect("create expired");
        expired_ids.push(expired.id);
    }
    let deleted = store
        .delete_expired_records(BATCH_SIZE)
        .await
        .expect("the batched delete runs on this backend");
    assert!(
        deleted >= EXPIRED_RECORDS as u64,
        "every expired record is deleted across batches, got {deleted}"
    );
    assert!(
        store.load(&first.id).await.expect("load").is_some(),
        "a live record survives the sweep"
    );
    for id in expired_ids {
        assert!(store.load(&id).await.expect("load").is_none());
    }
}
