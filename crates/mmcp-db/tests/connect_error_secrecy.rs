#![allow(clippy::unwrap_used, clippy::expect_used)]
//! A failed connect never carries the password of the database URL in its error chain.

use std::error::Error;

use mmcp_db::{DbError, connect};

const SENTINEL: &str = "SENTINEL_PASSWORD_7f3a";

/// The text of `error` and of every source behind it, joined.
fn chain_text(error: &(dyn Error + 'static)) -> String {
    let mut texts = vec![format!("{error} | {error:?}")];
    let mut source = error.source();
    while let Some(cause) = source {
        texts.push(format!("{cause} | {cause:?}"));
        source = cause.source();
    }
    texts.join("\n")
}

async fn failed_connect(url: &str) -> DbError {
    connect(url)
        .await
        .expect_err("the connect is expected to fail")
}

#[tokio::test]
async fn an_unparsable_url_is_refused_without_its_password() {
    let error = failed_connect(&format!("postgres://mmcp:{SENTINEL}@[bad-host/app")).await;

    let text = chain_text(&error);
    assert!(!text.contains(SENTINEL), "{text}");
    assert!(
        matches!(error, DbError::DatabaseUrlUnparsable { .. }),
        "{text}"
    );
}

#[tokio::test]
async fn a_misspelled_scheme_is_refused_without_its_password() {
    let error = failed_connect(&format!("postgress://mmcp:{SENTINEL}@db/app")).await;

    let text = chain_text(&error);
    assert!(!text.contains(SENTINEL), "{text}");
    match error {
        DbError::DatabaseUrlSchemeUnsupported { scheme, .. } => assert_eq!(scheme, "postgress"),
        other => panic!("unexpected error: {other:?}"),
    }
}

#[tokio::test]
async fn a_rejected_option_carries_no_password() {
    let error = failed_connect(&format!(
        "postgres://mmcp:{SENTINEL}@db/app?sslmode=bogus&password={SENTINEL}"
    ))
    .await;

    let text = chain_text(&error);
    assert!(!text.contains(SENTINEL), "{text}");
}

#[tokio::test]
async fn the_error_shows_the_redacted_url() {
    let error = failed_connect(&format!("postgress://mmcp:{SENTINEL}@db/app")).await;

    assert!(
        error.to_string().contains("postgress://mmcp@db/app"),
        "{error}"
    );
}
