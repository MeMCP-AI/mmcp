#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Sessions, passkey ceremonies and OAuth flows survive a server restart on a persistent database.

use std::net::SocketAddr;
use std::path::Path;

use mmcp_server::config::ServerConfig;
use mmcp_server::state::ServerState;
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
    common::TestServerConfigBuilder::new(tmp.path().to_path_buf())
        .database_url(sqlite_file_url(&tmp.path().join("mmcp.db")))
        .token_key(FIXED_TOKEN_KEY)
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
