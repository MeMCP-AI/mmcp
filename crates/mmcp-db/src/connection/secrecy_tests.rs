//! The database password never reaches a log or an error, whatever the form of the URL.
//!
//! Tracing is captured at trace level with span fields, and every printable form of every error is searched for a sentinel.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::error::Error;
use std::path::PathBuf;
use std::process;
use std::str::FromStr;
use std::time::Duration;

use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use sea_orm::sqlx::postgres::PgConnectOptions;

use super::connect::connect;
use super::database_target::DatabaseTarget;
use super::database_url_error::DatabaseUrlError;
use super::fake_peer::{Behavior, FakePeer};
use super::log_capture::LogCapture;
use super::parameter_set::ParameterSet;
use super::postgres_parameters::{ACCEPTED_KEYS, OPTIONS_FAMILY_PREFIX, SSL_MODE_KEYS};
use super::sqlite_parameters::{CACHE_VALUES, MODE_VALUES};
use crate::error::DbError;

const SENTINEL: &str = "SENTINEL_PASSWORD";

/// Longest wait for a connect to a fake peer, which ends it with an error at once.
/// Only a hang makes a test wait this long, and the wait then fails the test.
const CONNECT_WAIT: Duration = Duration::from_secs(20);

/// Longest wait for a connect that a test does not expect to end, to a host that does not answer.
const UNANSWERED_CONNECT_WAIT: Duration = Duration::from_millis(300);

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

/// A file in the temporary directory, removed when it drops.
struct TempFile(PathBuf);

impl TempFile {
    fn write(name: &str, content: &str) -> Self {
        let path = std::env::temp_dir().join(format!("mmcp-db-secrecy-{}-{name}", process::id()));
        std::fs::write(&path, content).unwrap();
        Self(path)
    }

    /// The path as the value of a URL parameter.
    fn as_parameter_value(&self) -> String {
        utf8_percent_encode(&self.0.to_string_lossy(), NON_ALPHANUMERIC).to_string()
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Postgres URLs that are accepted, each holding the sentinel where a secret can be, aimed at `port` of the loopback address.
fn accepted_postgres_urls(port: u16) -> Vec<String> {
    let s = SENTINEL;
    let host = format!("127.0.0.1:{port}");
    vec![
        format!("postgres://mmcp:{s}@{host}/app"),
        format!("postgresql://mmcp:{s}@{host}/app"),
        format!("postgres://mmcp:{s}%40x@{host}/app"),
        format!("postgres://{host}/app?password={s}&sslmode=disable"),
        format!("postgres://mmcp:{s}@{host}/app?password={s}"),
        format!("postgres://mmcp@{host}/app?options=-c%20x%3D{s}"),
        format!("postgres://mmcp@{host}/app?options[y]={s}"),
        format!("postgres://mmcp@{host}/app?sslkey={s}&user={s}"),
        format!("postgres://{host}/app?pass%77ord={s}"),
    ]
}

/// SQLite URLs refused at parse, each holding the sentinel where a secret can be, with the refusal that each draws.
fn refused_sqlite_urls() -> Vec<(String, DatabaseUrlError)> {
    let s = SENTINEL;
    vec![
        (
            format!("sqlite://u:{s}@data/x.db"),
            DatabaseUrlError::SqliteAtSignInFileName,
        ),
        (
            format!("sqlite:u:{s}@x.db"),
            DatabaseUrlError::SqliteAtSignInFileName,
        ),
        (
            format!("sqlite://alice%40{s}@archive/data.db"),
            DatabaseUrlError::SqliteAtSignInFileName,
        ),
        (
            format!("sqlite://u:#{s}/db"),
            DatabaseUrlError::SqliteFragmentPresent,
        ),
        (
            format!("sqlite:x.db#{s}"),
            DatabaseUrlError::SqliteFragmentPresent,
        ),
        (
            format!("sqlite:x.db?{s}=1"),
            DatabaseUrlError::UnrecognizedParameter {
                set: ParameterSet::Sqlite,
                position: 1,
                count: 1,
            },
        ),
        (
            format!("sqlite:x.db?{s}"),
            DatabaseUrlError::UnrecognizedParameter {
                set: ParameterSet::Sqlite,
                position: 1,
                count: 1,
            },
        ),
        (
            format!("sqlite:x.db?mode=rwc&password={s}"),
            DatabaseUrlError::UnrecognizedParameter {
                set: ParameterSet::Sqlite,
                position: 2,
                count: 2,
            },
        ),
        (
            format!("sqlite:x.db?cache={s}"),
            DatabaseUrlError::InvalidParameterValue {
                key: "cache",
                accepted: &CACHE_VALUES,
            },
        ),
        (
            format!("sqlite:x.db?mode={s}"),
            DatabaseUrlError::InvalidParameterValue {
                key: "mode",
                accepted: &MODE_VALUES,
            },
        ),
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
    let peer = FakePeer::start(Behavior::Closes).await;

    for url in accepted_postgres_urls(peer.port()) {
        let target = DatabaseTarget::from_str(&url).expect("the URL is accepted");
        assert!(
            !format!("{target} | {target:?}").contains(SENTINEL),
            "{url}"
        );
        let error = connect_error(&target)
            .await
            .unwrap_or_else(|| panic!("{url}: the connect to a closing peer must fail"));
        assert!(
            matches!(error, DbError::ConnectionFailed(_)),
            "{url}: the failure must come from the connection: {error}"
        );
        assert!(
            error.source().is_some(),
            "{url}: the chain searched holds the driver error"
        );
        assert!(!chain_text(&error).contains(SENTINEL), "{url}");
    }

    let logged = capture.text();
    assert!(!logged.contains(SENTINEL), "{logged}");
}

/// The error that a connect to `target` ends with, within [`CONNECT_WAIT`].
/// A connect that still runs after the wait is a hang, and fails the test.
async fn connect_error(target: &DatabaseTarget) -> Option<DbError> {
    let outcome = tokio::time::timeout(CONNECT_WAIT, connect(target))
        .await
        .expect("the connect ends within the wait");
    outcome.err()
}

/// The pairing check of the driver runs after the SSL request is answered, so a peer that answers it reaches the mapping of `connect`.
#[tokio::test]
async fn a_client_certificate_without_its_key_is_named_and_the_driver_error_is_dropped() {
    let (capture, _guard) = LogCapture::install();
    let peer = FakePeer::start(Behavior::AnswersSslRequestThenCloses).await;
    let certificate = TempFile::write("pairing-cert.pem", "not a certificate");
    let url = format!(
        "postgres://mmcp:{SENTINEL}@127.0.0.1:{}/app?sslmode=require&sslcert={}",
        peer.port(),
        certificate.as_parameter_value()
    );
    let target = DatabaseTarget::from_str(&url).expect("the URL is accepted");

    let error = connect_error(&target).await.expect("the connect fails");

    assert!(
        matches!(error, DbError::ClientCertificateAndKeyNotPaired),
        "{error:?}"
    );
    assert!(!chain_text(&error).contains(SENTINEL));
    assert!(!capture.text().contains(SENTINEL));
}

/// The key file holds the sentinel, so a driver error that quotes a line of it would show.
#[tokio::test]
async fn an_unreadable_client_key_is_named_and_the_driver_error_is_dropped() {
    let (capture, _guard) = LogCapture::install();
    let peer = FakePeer::start(Behavior::AnswersSslRequestThenCloses).await;
    let certificate = TempFile::write("key-cert.pem", "not a certificate");
    let key = TempFile::write(
        "key.pem",
        &format!("-----BEGIN PRIVATE KEY-----\n{SENTINEL}\n-----END PRIVATE KEY-----\n"),
    );
    let url = format!(
        "postgres://mmcp:{SENTINEL}@127.0.0.1:{}/app?sslmode=require&sslcert={}&sslkey={}",
        peer.port(),
        certificate.as_parameter_value(),
        key.as_parameter_value()
    );
    let target = DatabaseTarget::from_str(&url).expect("the URL is accepted");

    let error = connect_error(&target).await.expect("the connect fails");

    assert!(matches!(error, DbError::ClientKeyRejected), "{error:?}");
    assert!(!chain_text(&error).contains(SENTINEL));
    assert!(!capture.text().contains(SENTINEL));
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
                let _ = tokio::time::timeout(UNANSWERED_CONNECT_WAIT, connect(&target)).await;
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

#[tokio::test]
async fn a_refused_sqlite_url_leaves_no_sentinel_in_the_error_or_the_trace_logs() {
    let (capture, _guard) = LogCapture::install();

    for (url, expected) in refused_sqlite_urls() {
        let error = DatabaseTarget::from_str(&url).expect_err(&url);

        assert_eq!(error, expected, "{url}");
        assert!(!chain_text(&error).contains(SENTINEL), "{url}");
    }

    assert!(!capture.text().contains(SENTINEL), "{}", capture.text());
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
