#![allow(clippy::unwrap_used, clippy::expect_used)]
//! A credential change signs the account's other sessions out, and logins never sign each other out.
//!
//! The passkey ceremony needs an authenticator, so the credential changes here go through the repository writers
//! the routes call, against the database the running server reads.

use std::net::SocketAddr;

use mmcp_db::repository::oauth_repo::NewOauthAccount;
use mmcp_db::repository::{oauth_repo, passkey_repo, user_repo};
use mmcp_server::config::ServerConfig;
use mmcp_server::state::ServerState;
use serde_json::json;
use tempfile::TempDir;
use tokio::task::JoinHandle;
use uuid::Uuid;

mod common;

const PASSWORD: &str = "hunter22hunter22";
const REPLACEMENT_PASSWORD: &str = "replacement-passphrase-77";

/// Provider user id the fake OAuth provider returns for every login.
const FAKE_PROVIDER_USER_ID: &str = "42";

/// Handle the backend provisions for a first-time login of [`FAKE_PROVIDER_USER_ID`].
const FIRST_LOGIN_HANDLE: &str = "github_42";

struct TestServer {
    addr: SocketAddr,
    cfg: ServerConfig,
    task: JoinHandle<()>,
    _tmp: TempDir,
}

impl TestServer {
    async fn start() -> Self {
        let tmp = TempDir::new().expect("tempdir");
        let (fake_addr, _probes) = common::start_fake_oauth_provider().await;
        let db_path = tmp
            .path()
            .join("mmcp.db")
            .to_string_lossy()
            .replace('\\', "/");
        let cfg = common::TestServerConfigBuilder::new(tmp.path().to_path_buf())
            .database_url(format!("sqlite://{db_path}?mode=rwc"))
            .oauth_providers(vec![common::fake_oauth_provider_config(fake_addr)])
            .build();
        let state = ServerState::initialize(&cfg).await.expect("state init");
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
        Self {
            addr,
            cfg,
            task,
            _tmp: tmp,
        }
    }

    async fn database(&self) -> mmcp_db::Database {
        mmcp_db::connect(&self.cfg.database_url)
            .await
            .expect("connect to the server's database")
    }

    async fn user_id(&self, handle: &str) -> Uuid {
        user_repo::find_by_handle(self.database().await.connection(), handle)
            .await
            .expect("find user")
            .expect("user exists")
            .id
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn browser() -> reqwest::Client {
    reqwest::Client::builder()
        .cookie_store(true)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("build client")
}

async fn register(server: &TestServer, handle: &str) {
    let response = reqwest::Client::new()
        .post(format!("http://{}/auth/register", server.addr))
        .json(&json!({ "handle": handle, "password": PASSWORD }))
        .send()
        .await
        .expect("register");
    assert_eq!(response.status(), 201);
}

async fn password_login(
    client: &reqwest::Client,
    server: &TestServer,
    handle: &str,
    password: &str,
) {
    let response = client
        .post(format!("http://{}/auth/login", server.addr))
        .json(&json!({ "handle": handle, "password": password }))
        .send()
        .await
        .expect("login");
    assert_eq!(response.status(), 200, "password login of {handle}");
}

/// Completes an OAuth login against the fake provider; the session cookie lands in the client's jar.
async fn oauth_login(client: &reqwest::Client, server: &TestServer) {
    let authorize = client
        .get(format!(
            "http://{}/auth/oauth/github/authorize",
            server.addr
        ))
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
        .expect("a state query parameter")
        .split('&')
        .next()
        .unwrap_or_default()
        .to_owned();
    let callback = client
        .get(format!(
            "http://{}/auth/oauth/github/callback?code=fake-code&state={state}",
            server.addr
        ))
        .send()
        .await
        .expect("oauth callback");
    assert_eq!(callback.status(), 200, "oauth login");
}

/// Status of a route that requires an authenticated session: 200 when signed in, 401 when not.
async fn authenticated_status(
    client: &reqwest::Client,
    server: &TestServer,
) -> reqwest::StatusCode {
    client
        .post(format!(
            "http://{}/auth/passkey/register/start",
            server.addr
        ))
        .send()
        .await
        .expect("authenticated route")
        .status()
}

async fn add_passkey(server: &TestServer, user_id: Uuid) -> Uuid {
    let passkey_id = Uuid::now_v7();
    passkey_repo::create(
        server.database().await.connection(),
        passkey_id,
        user_id,
        "key".to_owned(),
        "{}".to_owned(),
        0,
    )
    .await
    .expect("add a passkey");
    passkey_id
}

async fn link_oauth_account(server: &TestServer, user_id: Uuid) {
    oauth_repo::create(
        server.database().await.connection(),
        NewOauthAccount {
            id: Uuid::now_v7(),
            user_id,
            provider: "github".to_owned(),
            provider_user_id: FAKE_PROVIDER_USER_ID.to_owned(),
            email: None,
            access_token: None,
            refresh_token: None,
            created_at: 0,
        },
    )
    .await
    .expect("link the oauth account");
}

#[tokio::test]
async fn adding_a_passkey_signs_out_the_other_sessions_of_an_oauth_only_user() {
    let server = TestServer::start().await;
    let other_browser = browser();
    oauth_login(&other_browser, &server).await;
    assert_eq!(authenticated_status(&other_browser, &server).await, 200);

    let user_id = server.user_id(FIRST_LOGIN_HANDLE).await;
    add_passkey(&server, user_id).await;

    assert_eq!(
        authenticated_status(&other_browser, &server).await,
        401,
        "a passkey added elsewhere must sign this session out"
    );
}

#[tokio::test]
async fn first_time_oauth_login_stays_signed_in_on_the_next_request() {
    let server = TestServer::start().await;
    let client = browser();

    oauth_login(&client, &server).await;

    assert_eq!(
        authenticated_status(&client, &server).await,
        200,
        "the login that created the account must not sign itself out"
    );
    assert_eq!(
        authenticated_status(&client, &server).await,
        200,
        "and it stays signed in on the request after that"
    );
}

#[tokio::test]
async fn password_login_elsewhere_keeps_the_first_session() {
    let server = TestServer::start().await;
    register(&server, "alice").await;
    let first = browser();
    let second = browser();

    password_login(&first, &server, "alice", PASSWORD).await;
    password_login(&second, &server, "alice", PASSWORD).await;

    assert_eq!(authenticated_status(&first, &server).await, 200);
    assert_eq!(authenticated_status(&second, &server).await, 200);
}

#[tokio::test]
async fn oauth_login_elsewhere_keeps_the_first_session() {
    let server = TestServer::start().await;
    register(&server, "alice").await;
    link_oauth_account(&server, server.user_id("alice").await).await;
    let first = browser();
    let second = browser();
    password_login(&first, &server, "alice", PASSWORD).await;

    oauth_login(&second, &server).await;

    assert_eq!(authenticated_status(&first, &server).await, 200);
    assert_eq!(authenticated_status(&second, &server).await, 200);
}

#[tokio::test]
async fn each_credential_change_signs_out_old_sessions_and_a_new_login_works() {
    let server = TestServer::start().await;
    register(&server, "alice").await;
    let alice = server.user_id("alice").await;
    let mut password = PASSWORD;

    for change in [
        "passkey added",
        "passkey removed",
        "oauth linked",
        "password set",
    ] {
        let old_session = browser();
        password_login(&old_session, &server, "alice", password).await;
        assert_eq!(authenticated_status(&old_session, &server).await, 200);

        let database = server.database().await;
        match change {
            "passkey added" => {
                add_passkey(&server, alice).await;
            }
            "passkey removed" => {
                let passkey = passkey_repo::find_by_user(database.connection(), alice)
                    .await
                    .expect("list passkeys")
                    .pop()
                    .expect("the earlier step added a passkey");
                passkey_repo::delete(database.connection(), passkey.id)
                    .await
                    .expect("remove the passkey");
            }
            "oauth linked" => link_oauth_account(&server, alice).await,
            _ => {
                let hash = mmcp_auth::hash_password(REPLACEMENT_PASSWORD).expect("hash");
                user_repo::update_profile(database.connection(), alice, None, Some(hash))
                    .await
                    .expect("set the password");
                password = REPLACEMENT_PASSWORD;
            }
        }

        assert_eq!(
            authenticated_status(&old_session, &server).await,
            401,
            "{change}: the session that predates the change must be signed out"
        );
        let new_session = browser();
        password_login(&new_session, &server, "alice", password).await;
        assert_eq!(
            authenticated_status(&new_session, &server).await,
            200,
            "{change}: a login after the change must be authenticated"
        );
    }
}
