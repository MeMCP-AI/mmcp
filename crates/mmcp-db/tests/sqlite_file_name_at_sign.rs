#![allow(clippy::unwrap_used, clippy::expect_used)]
//! A SQLite file whose name holds an `@` opens through its percent-encoded URL.

use mmcp_db::{DatabaseTarget, DatabaseUrlError, connect};
use uuid::Uuid;

/// A `sqlite://` URL for a fresh file inside the temp directory, with `file_name` percent-encoded as given.
fn temp_url(encoded_file_name: &str) -> (std::path::PathBuf, String) {
    let directory = std::env::temp_dir().join(format!("mmcp-db-at-sign-{}", Uuid::now_v7()));
    std::fs::create_dir_all(&directory).unwrap();
    let url_directory = directory.to_string_lossy().replace('\\', "/");
    (
        directory,
        format!("sqlite://{url_directory}/{encoded_file_name}?mode=rwc"),
    )
}

#[tokio::test]
async fn a_file_name_holding_an_at_sign_opens_through_its_encoded_url() {
    let (directory, url) = temp_url("alice%40example.db");

    let target = url
        .parse::<DatabaseTarget>()
        .expect("the encoded URL is accepted");
    let database = connect(&target).await.expect("the file opens");
    database.migrate().await.expect("migrations run");

    assert!(directory.join("alice@example.db").is_file());
    drop(database);
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn the_raw_at_sign_is_refused_with_a_remedy_that_works_for_sqlite() {
    let (directory, url) = temp_url("alice@example.db");

    let error = url.parse::<DatabaseTarget>().unwrap_err();

    assert_eq!(error, DatabaseUrlError::SqliteAtSignInFileName);
    assert!(error.to_string().contains("%40"));
    let _ = std::fs::remove_dir_all(directory);
}
