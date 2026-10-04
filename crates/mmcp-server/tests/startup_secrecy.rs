#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Integration test: no database password reaches stdout or stderr of the server binary, on any startup path.

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const SENTINEL: &str = "SENTINEL_STARTUP_PASSWORD";

/// A fixed signing key, 64 hexadecimal characters.
const TOKEN_KEY_HEX: &str = "1111111111111111111111111111111111111111111111111111111111111111";

/// Longest wait for a refused configuration to end the process.
const REFUSAL_WAIT: Duration = Duration::from_secs(20);

/// How long a server that cannot reach its database runs before it is stopped.
const RUN_WINDOW: Duration = Duration::from_millis(2500);

const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// What a run of the binary printed, and whether it ended by itself.
struct Run {
    printed: String,
    ended_by_itself_with_failure: bool,
}

/// Starts `mmcp-server serve` with `database_url`, optionally under `rust_log`, and stops it after `window` at most.
fn run_server(database_url: &str, rust_log: Option<&str>, window: Duration) -> Run {
    let repo_root = tempfile::tempdir().unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_mmcp-server"));
    command
        .arg("serve")
        .env("MMCP_REPO_ROOT", repo_root.path())
        .env("MMCP_BIND", "127.0.0.1:0")
        .env("MMCP_TOKEN_KEY_HEX", TOKEN_KEY_HEX)
        .env("MMCP_DATABASE_URL", database_url)
        .env("NO_COLOR", "1")
        .env_remove("RUST_LOG")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(filter) = rust_log {
        command.env("RUST_LOG", filter);
    }
    let mut child = command.spawn().unwrap();

    let started = Instant::now();
    let mut ended_by_itself_with_failure = false;
    while started.elapsed() < window {
        if let Some(status) = child.try_wait().unwrap() {
            ended_by_itself_with_failure = !status.success();
            break;
        }
        std::thread::sleep(POLL_INTERVAL);
    }
    let _ = child.kill();
    let output = child.wait_with_output().unwrap();
    Run {
        printed: format!(
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ),
        ended_by_itself_with_failure,
    }
}

const FILTERS: [Option<&str>; 2] = [None, Some("trace")];

#[test]
fn a_refused_database_url_ends_the_start_without_printing_the_password() {
    let refused = [
        format!("postgres:/mmcp:{SENTINEL}@db/app"),
        format!("postgres:mmcp:{SENTINEL}@db/app"),
        format!("postgres://mmcp:/{SENTINEL}@db/app"),
        format!("postgres://mmcp:#{SENTINEL}@db/app"),
        format!("postgres://mmcp@db/app?PassWord={SENTINEL}"),
        format!("postgres://db/app?password=ab&{SENTINEL}"),
        format!("postgress://mmcp:{SENTINEL}@db/app"),
        format!("postgres://mmcp:{SENTINEL}@[bad-host/app"),
        format!("sqlite://u:{SENTINEL}@data/x.db"),
    ];
    for url in &refused {
        for filter in FILTERS {
            let run = run_server(url, filter, REFUSAL_WAIT);

            assert!(
                !run.printed.contains(SENTINEL),
                "{url} under {filter:?}: {}",
                run.printed
            );
            assert!(
                run.ended_by_itself_with_failure,
                "{url} must end the start with a failure"
            );
        }
    }
}

#[test]
fn an_accepted_postgres_url_that_cannot_connect_prints_no_password() {
    let accepted = [
        format!("postgres://mmcp:{SENTINEL}@127.0.0.1:1/app"),
        format!("postgres://127.0.0.1:1/app?password={SENTINEL}&sslmode=disable"),
    ];
    for url in &accepted {
        for filter in FILTERS {
            let run = run_server(url, filter, RUN_WINDOW);

            assert!(
                run.printed.contains("mmcp-server starting"),
                "{url} must reach the startup line: {}",
                run.printed
            );
            assert!(
                !run.printed.contains(SENTINEL),
                "{url} under {filter:?}: {}",
                run.printed
            );
        }
    }
}

#[test]
fn an_in_memory_sqlite_url_is_shown_as_written_and_the_server_listens() {
    let run = run_server("sqlite::memory:", None, RUN_WINDOW);

    assert!(
        run.printed.contains("database=sqlite::memory:"),
        "{}",
        run.printed
    );
    assert!(run.printed.contains("listening"), "{}", run.printed);
}
