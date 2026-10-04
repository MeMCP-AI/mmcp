#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Integration test: a failed startup never prints the database password.

use mmcp_server::config::test_support::minimal_server_config;
use mmcp_server::state::ServerState;

const SENTINEL: &str = "SENTINEL_PASSWORD_91bc";

/// What `main` prints for a failed startup: the error with its whole cause chain.
async fn startup_error_text(database_url: &str) -> String {
    let mut cfg = minimal_server_config(tempfile::tempdir().unwrap().keep());
    cfg.database_url = database_url.to_owned();
    let error = ServerState::initialize(&cfg)
        .await
        .err()
        .expect("startup is expected to fail");
    format!("{error:#} | {error:?}")
}

#[tokio::test]
async fn an_unparsable_database_url_leaves_no_password_in_the_startup_error() {
    let text = startup_error_text(&format!("postgres://mmcp:{SENTINEL}@[bad-host/app")).await;

    assert!(!text.contains(SENTINEL), "{text}");
}

#[tokio::test]
async fn a_misspelled_scheme_leaves_no_password_in_the_startup_error() {
    let text = startup_error_text(&format!("postgress://mmcp:{SENTINEL}@db/app")).await;

    assert!(!text.contains(SENTINEL), "{text}");
}
