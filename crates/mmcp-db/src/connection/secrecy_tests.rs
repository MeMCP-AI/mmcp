//! The database password never reaches a log or an error, whatever the form of the URL.
//!
//! Tracing is captured at trace level with span fields, and every printable form of every error is searched for a sentinel.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::error::Error;
use std::str::FromStr;
use std::time::Duration;

use sea_orm::sqlx::postgres::PgConnectOptions;

use super::connect::connect;
use super::database_target::DatabaseTarget;
use super::log_capture::LogCapture;
use super::postgres_parameters::{ACCEPTED_KEYS, OPTIONS_FAMILY_PREFIX, SSL_MODE_KEYS};

const SENTINEL: &str = "SENTINEL_PASSWORD";

/// Longest wait for a connect to a port nobody listens on; the options are parsed on the first poll.
const CONNECT_WAIT: Duration = Duration::from_millis(300);

/// Every printable form of `error` and of each source behind it.
fn chain_text(error: &(dyn Error + 'static)) -> String {
    let mut texts = vec![format!("{error} | {error:?}")];
    let mut source = error.source();
    while let Some(cause) = source {
        texts.push(format!("{cause} | {cause:?}"));
        source = cause.source();
    }
    texts.join("\n")
}

/// Postgres URLs that are accepted, each holding the sentinel where a secret can be, aimed at a closed port.
fn accepted_postgres_urls() -> Vec<String> {
    let s = SENTINEL;
    vec![
        format!("postgres://mmcp:{s}@127.0.0.1:1/app"),
        format!("postgresql://mmcp:{s}@127.0.0.1:1/app"),
        format!("postgres://mmcp:{s}%40x@127.0.0.1:1/app"),
        format!("postgres://127.0.0.1:1/app?password={s}&sslmode=disable"),
        format!("postgres://mmcp:{s}@127.0.0.1:1/app?password={s}"),
        format!("postgres://mmcp@127.0.0.1:1/app?options=-c%20x%3D{s}"),
        format!("postgres://mmcp@127.0.0.1:1/app?options[y]={s}"),
        format!("postgres://mmcp@127.0.0.1:1/app?sslkey={s}&user={s}"),
        format!("postgres://127.0.0.1:1/app?pass%77ord={s}"),
    ]
}

/// URLs refused at parse, each holding the sentinel where a secret can be.
fn refused_urls() -> Vec<String> {
    let s = SENTINEL;
    vec![
        format!("postgres:/mmcp:{s}@db/app"),
        format!("postgres:mmcp:{s}@db/app"),
        format!("postgres://mmcp:#{s}@db/app"),
        format!("postgres://mmcp:/{s}@db/app"),
        format!("postgres://mmcp:?{s}@db/app"),
        format!("postgres://mmcp:1?k={s}@db/app"),
        format!("postgres:///user:{s}%40host/db"),
        format!("postgres://mmcp@db/app?{s}"),
        format!("postgres://db/app?password=ab&{s}"),
        format!("postgres://db/app?password=ab&{s}=ef"),
        format!("postgres://mmcp@db/app?PassWord={s}"),
        format!("postgres://mmcp@db/app?passwd={s}"),
        format!("postgres://mmcp@db/app?sslpassword={s}"),
        format!("postgres://db/app?password=ab&sslmode={s}"),
        format!("postgress://mmcp:{s}@db/app"),
        format!("postgress:/mmcp:{s}@db/app"),
        format!("postgres://mmcp:{s}@[bad-host/app"),
        format!("postgres://mmcp:{s}@/app"),
        format!("sqlite://u:{s}@data/x.db"),
        format!("sqlite:u:{s}@x.db"),
    ]
}

#[tokio::test]
async fn an_accepted_postgres_url_leaves_no_password_in_the_trace_logs_or_the_error() {
    let (capture, _guard) = LogCapture::install();

    for url in accepted_postgres_urls() {
        let target = DatabaseTarget::from_str(&url).expect("the URL is accepted");
        assert!(
            !format!("{target} | {target:?}").contains(SENTINEL),
            "{url}"
        );
        if let Ok(Err(error)) = tokio::time::timeout(CONNECT_WAIT, connect(&target)).await {
            assert!(!chain_text(&error).contains(SENTINEL), "{url}");
        }
    }

    let logged = capture.text();
    assert!(!logged.contains(SENTINEL), "{logged}");
}

#[tokio::test]
async fn a_refused_url_leaves_no_password_in_the_trace_logs_or_the_error() {
    let (capture, _guard) = LogCapture::install();

    let mut wrongly_accepted = Vec::new();
    for url in refused_urls() {
        match DatabaseTarget::from_str(&url) {
            Err(error) => assert!(!chain_text(&error).contains(SENTINEL), "{url}"),
            Ok(target) => {
                // A URL that should have been refused is connected, to see what the dependencies would log.
                let _ = tokio::time::timeout(CONNECT_WAIT, connect(&target)).await;
                wrongly_accepted.push(url);
            }
        }
    }

    let logged = capture.text();
    assert!(!logged.contains(SENTINEL), "{logged}");
    assert!(wrongly_accepted.is_empty(), "{wrongly_accepted:?}");
}

#[tokio::test]
async fn a_sqlite_url_connects_and_shows_in_the_trace_logs_only_what_it_says() {
    let (capture, _guard) = LogCapture::install();
    let target = DatabaseTarget::from_str("sqlite::memory:").expect("accepted");

    let database = connect(&target).await.expect("an in-memory database opens");
    database.migrate().await.expect("migrations run");

    assert!(!capture.text().contains(SENTINEL));
}

/// The harness must see a secret where a dependency puts one, or a clean capture proves nothing.
#[tokio::test]
async fn the_capture_sees_the_sqlx_warn_that_a_misspelled_parameter_triggers() {
    let (capture, _guard) = LogCapture::install();

    let parsed = PgConnectOptions::from_str(&format!("postgres://h/d?PassWord={SENTINEL}"));

    assert!(parsed.is_ok());
    assert!(
        capture.text().contains(SENTINEL),
        "the sqlx warn names the value of an unrecognized parameter"
    );
}

#[tokio::test]
async fn the_capture_sees_the_url_in_the_span_that_seaorm_connect_opens() {
    let (capture, _guard) = LogCapture::install();

    let _ = sea_orm::Database::connect(format!("sqlite::memory:?x={SENTINEL}")).await;

    assert!(
        capture.text().contains(SENTINEL),
        "the SeaORM connect span records its options, URL included"
    );
}

/// A value of each accepted key that sqlx takes.
fn valid_value(key: &str) -> &'static str {
    match key {
        "sslmode" | "ssl-mode" => "require",
        "statement-cache-capacity" | "port" => "10",
        "hostaddr" => "127.0.0.1",
        "options" => "-c x=y",
        _ => "x",
    }
}

#[tokio::test]
async fn sqlx_recognizes_every_mirrored_key_and_warns_for_any_other() {
    let (capture, _guard) = LogCapture::install();

    for key in ACCEPTED_KEYS {
        let url = format!("postgres://h/d?{key}={}", valid_value(key));
        PgConnectOptions::from_str(&url).unwrap_or_else(|error| panic!("{key}: {error}"));
    }
    PgConnectOptions::from_str(&format!("postgres://h/d?{OPTIONS_FAMILY_PREFIX}y]=z")).unwrap();
    assert!(
        !capture.text().contains("ignoring unrecognized"),
        "{}",
        capture.text()
    );

    PgConnectOptions::from_str("postgres://h/d?not-a-key=v").unwrap();
    assert!(capture.text().contains("ignoring unrecognized"));
}

#[test]
fn no_sqlx_parse_error_of_an_accepted_key_quotes_its_value_unless_mmcp_checks_the_value_first() {
    for key in ACCEPTED_KEYS {
        let url = format!("postgres://h/d?{key}={SENTINEL}");
        let Err(error) = PgConnectOptions::from_str(&url) else {
            continue;
        };
        let quotes_the_value = chain_text(&error).contains(SENTINEL);
        assert!(
            !quotes_the_value || SSL_MODE_KEYS.contains(&key),
            "{key} quotes its value in the sqlx error and mmcp does not check it first"
        );
    }
}
