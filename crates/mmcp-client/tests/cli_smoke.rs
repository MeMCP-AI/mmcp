#![allow(clippy::unwrap_used, clippy::expect_used)]
//! CLI smoke tests via `assert_cmd`.
//!
//! These tests invoke the compiled `mmcp` binary and assert on its
//! exit code and output without a running server.

use assert_cmd::Command;
use predicates::prelude::*;

fn mmcp() -> Command {
    Command::cargo_bin("mmcp").expect("mmcp binary")
}

#[test]
fn no_args_prints_help() {
    mmcp()
        .assert()
        .failure()
        .stderr(predicate::str::contains("Usage"));
}

#[test]
fn help_flag_succeeds() {
    mmcp()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("mmcp"));
}

#[test]
fn bare_init_prints_help_instead_of_hard_error() {
    // `mmcp init` without a subcommand should render help text for
    // the `init` group, not a terse "required subcommand" abort.
    // Clap's `arg_required_else_help` emits the usage block on
    // stderr and exits non-zero, same as `mmcp` alone.
    mmcp()
        .arg("init")
        .assert()
        .failure()
        .stderr(predicate::str::contains("Usage"))
        .stderr(predicate::str::contains("project"))
        .stderr(predicate::str::contains("claude"));
}

#[test]
fn init_project_config_only_creates_config_without_repo() {
    let tmp = tempfile::tempdir().unwrap();
    let mmcp_home = tmp.path().join("mmcp-home");
    mmcp()
        .args(["init", "project", "--slug", "cli-smoke", "--config-only"])
        .current_dir(tmp.path())
        .env("MMCP_HOME", &mmcp_home)
        .assert()
        .success();

    let config_path = tmp.path().join(".mmcp.toml");
    assert!(
        config_path.exists(),
        "init project --config-only must create .mmcp.toml"
    );
    let body = std::fs::read_to_string(&config_path).expect("read config");
    assert!(
        body.contains("project_slug = \"cli-smoke\""),
        "slug must be stored in .mmcp.toml; got:\n{body}"
    );
}

#[test]
fn status_outside_project_fails() {
    let tmp = tempfile::tempdir().unwrap();
    mmcp()
        .arg("status")
        .current_dir(tmp.path())
        .assert()
        .failure();
}

#[test]
fn status_inside_initialized_project_prints_project_fields() {
    let tmp = tempfile::tempdir().unwrap();
    let mmcp_home = tmp.path().join("mmcp-home");
    mmcp()
        .args(["init", "project", "--slug", "status-test", "--config-only"])
        .current_dir(tmp.path())
        .env("MMCP_HOME", &mmcp_home)
        .assert()
        .success();

    mmcp()
        .arg("status")
        .current_dir(tmp.path())
        .env("MMCP_HOME", &mmcp_home)
        .assert()
        .success()
        .stdout(predicate::str::contains("project root"))
        .stdout(predicate::str::contains("project uuid"))
        .stdout(predicate::str::contains("server"))
        .stdout(predicate::str::contains("default group"));
}

#[test]
fn init_project_second_call_is_idempotent() {
    // `init project` never overwrites: a second run
    // against an already-initialized project must succeed and leave
    // the repo as-is, so the command is safe to run defensively.
    let tmp = tempfile::tempdir().unwrap();
    let mmcp_home = tmp.path().join("mmcp-home");
    mmcp()
        .args(["init", "project", "--slug", "idempotent"])
        .current_dir(tmp.path())
        .env("MMCP_HOME", &mmcp_home)
        .assert()
        .success();
    mmcp()
        .args(["init", "project", "--slug", "idempotent"])
        .current_dir(tmp.path())
        .env("MMCP_HOME", &mmcp_home)
        .assert()
        .success()
        .stdout(predicate::str::contains("already present"));
}

#[test]
fn sync_fails_when_project_has_no_sync_block() {
    // A freshly initialized project has an empty `[sync]` (no
    // legacy `server_url`, no `[[sync.remotes]]`) at either user or
    // project level, so running `mmcp sync --all` against it must
    // fail with a message naming the empty effective remote set.
    // Use `--config-only` to keep the tempdir free of the bare repo
    // - sync never gets that far anyway. `--all` is the explicit
    // whole-mirror selector; bare `mmcp sync` is its own failure
    // mode covered by `sync_rejects_bare_call_without_selector`
    // below.
    let tmp = tempfile::tempdir().unwrap();
    let mmcp_home = tmp.path().join("mmcp-home");
    mmcp()
        .args(["init", "project", "--slug", "sync-test", "--config-only"])
        .current_dir(tmp.path())
        .env("MMCP_HOME", &mmcp_home)
        .assert()
        .success();
    mmcp()
        .args(["sync", "--all"])
        .current_dir(tmp.path())
        .env("MMCP_HOME", &mmcp_home)
        .assert()
        .failure()
        .stderr(predicate::str::contains("no sync remotes configured"));
}

#[test]
fn sync_outside_project_fails_with_a_useful_message() {
    let tmp = tempfile::tempdir().unwrap();
    let mmcp_home = tmp.path().join("mmcp-home");
    mmcp()
        .args(["sync", "--all"])
        .current_dir(tmp.path())
        .env("MMCP_HOME", &mmcp_home)
        .assert()
        .failure()
        .stderr(predicate::str::contains("no mmcp project"));
}

#[test]
fn sync_rejects_bare_call_without_selector() {
    // Per the no-global-default invariant: `mmcp sync` without a
    // selector exits non-zero at the clap parse boundary with a
    // message naming the three accepted selectors so the operator
    // knows which one to pick.
    let tmp = tempfile::tempdir().unwrap();
    let mmcp_home = tmp.path().join("mmcp-home");
    mmcp()
        .arg("sync")
        .current_dir(tmp.path())
        .env("MMCP_HOME", &mmcp_home)
        .assert()
        .failure()
        .stderr(
            predicate::str::contains("--group")
                .and(predicate::str::contains("--scope").and(predicate::str::contains("--all"))),
        );
}

#[test]
fn hook_user_prompt_emits_session_marker() {
    let tmp = tempfile::tempdir().unwrap();
    let mmcp_home = tmp.path().join("mmcp-home");
    let payload = r#"{"session_id":"sess-cli-smoke-1"}"#;
    mmcp()
        .args(["hook", "user-prompt"])
        .env("MMCP_HOME", &mmcp_home)
        .write_stdin(payload)
        .assert()
        .success()
        .stdout(predicate::str::contains("sess-cli-smoke-1"))
        .stdout(predicate::str::contains("turn=#1"));
}

#[test]
fn hook_user_prompt_rejects_malformed_json() {
    let tmp = tempfile::tempdir().unwrap();
    let mmcp_home = tmp.path().join("mmcp-home");
    mmcp()
        .args(["hook", "user-prompt"])
        .env("MMCP_HOME", &mmcp_home)
        .write_stdin("not json at all")
        .assert()
        .failure()
        .stderr(predicate::str::contains("parsing hook payload"));
}

#[test]
fn init_claude_dry_run_against_override_announces_plan_without_writing() {
    let tmp = tempfile::tempdir().unwrap();
    let mmcp_home = tmp.path().join("mmcp-home");
    mmcp()
        .args(["init", "project", "--slug", "claude-test", "--config-only"])
        .current_dir(tmp.path())
        .env("MMCP_HOME", &mmcp_home)
        .assert()
        .success();

    mmcp()
        .args(["init", "claude", "--override", "--dry-run"])
        .current_dir(tmp.path())
        .env("MMCP_HOME", &mmcp_home)
        .assert()
        .success()
        .stderr(predicate::str::contains("dry-run"));

    assert!(
        !tmp.path().join("CLAUDE.md").exists(),
        "dry-run must not write the file"
    );
}

#[test]
fn bare_feature_prints_help_instead_of_hard_error() {
    // `mmcp feature` without a subcommand prints the subcommand
    // index rather than exiting with a terse "required subcommand"
    // message. Same contract as `mmcp init`.
    mmcp()
        .arg("feature")
        .assert()
        .failure()
        .stderr(predicate::str::contains("Usage"))
        .stderr(predicate::str::contains("add"))
        .stderr(predicate::str::contains("list"));
}

#[test]
fn feature_list_outside_project_reports_project_not_found() {
    // FR tools refuse to operate outside an initialised project
    // rather than silently falling back to a no-op. The error
    // message points the operator at the right fix (`init project`
    // or `cd` into a project dir).
    let tmp = tempfile::tempdir().unwrap();
    let mmcp_home = tmp.path().join("mmcp-home");
    mmcp()
        .args(["feature", "list"])
        .current_dir(tmp.path())
        .env("MMCP_HOME", &mmcp_home)
        .assert()
        .failure()
        .stderr(predicate::str::contains("no mmcp project"));
}

#[test]
fn feature_add_list_read_round_trips_inside_project() {
    let tmp = tempfile::tempdir().unwrap();
    let mmcp_home = tmp.path().join("mmcp-home");
    mmcp()
        .args(["init", "project", "--slug", "feature-smoke"])
        .current_dir(tmp.path())
        .env("MMCP_HOME", &mmcp_home)
        .assert()
        .success();

    mmcp()
        .args([
            "feature",
            "add",
            "--slug",
            "fr-first",
            "--title",
            "First FR",
            "--description",
            "smoke-test FR",
            "--body",
            "## Need\nTest the CLI FR path.\n",
        ])
        .current_dir(tmp.path())
        .env("MMCP_HOME", &mmcp_home)
        .assert()
        .success()
        .stdout(predicate::str::contains("created feature `fr-first`"))
        .stdout(predicate::str::contains("status: requested"));

    mmcp()
        .args(["feature", "list"])
        .current_dir(tmp.path())
        .env("MMCP_HOME", &mmcp_home)
        .assert()
        .success()
        .stdout(predicate::str::contains("fr-first"))
        .stdout(predicate::str::contains("[requested]"))
        .stdout(predicate::str::contains("1 feature"));

    mmcp()
        .args(["feature", "read", "fr-first"])
        .current_dir(tmp.path())
        .env("MMCP_HOME", &mmcp_home)
        .assert()
        .success()
        .stdout(predicate::str::contains("slug        : fr-first"))
        .stdout(predicate::str::contains("status      : requested"))
        .stdout(predicate::str::contains("Test the CLI FR path."));

    mmcp()
        .args(["feature", "update", "fr-first", "--status", "completed"])
        .current_dir(tmp.path())
        .env("MMCP_HOME", &mmcp_home)
        .assert()
        .success()
        .stdout(predicate::str::contains("updated feature `fr-first`"))
        .stdout(predicate::str::contains("status: completed"));

    mmcp()
        .args(["feature", "list", "--status", "requested"])
        .current_dir(tmp.path())
        .env("MMCP_HOME", &mmcp_home)
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "no feature requests with status `requested`",
        ));

    // Default listing hides closed-like FRs. The
    // completed entry above must drop out, and the help text has to
    // point operators at `--all` so the hide is self-documenting.
    mmcp()
        .args(["feature", "list"])
        .current_dir(tmp.path())
        .env("MMCP_HOME", &mmcp_home)
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "no open feature requests in this project; pass --all to include closed ones",
        ));

    // `--all` re-includes the completed FR with its status marker.
    mmcp()
        .args(["feature", "list", "--all"])
        .current_dir(tmp.path())
        .env("MMCP_HOME", &mmcp_home)
        .assert()
        .success()
        .stdout(predicate::str::contains("fr-first"))
        .stdout(predicate::str::contains("[completed]"))
        .stdout(predicate::str::contains("1 feature"));
}

#[test]
fn debug_cache_rebuild_forces_a_full_reindex() {
    // `init project` without `--config-only` creates a real local
    // bare group repo -- no remote needed -- so the rebuild below
    // has a real group to walk.
    let tmp = tempfile::tempdir().unwrap();
    let mmcp_home = tmp.path().join("mmcp-home");
    mmcp()
        .args(["init", "project", "--slug", "cache-rebuild-smoke"])
        .current_dir(tmp.path())
        .env("MMCP_HOME", &mmcp_home)
        .assert()
        .success();

    mmcp()
        .args(["debug", "cache-rebuild"])
        .current_dir(tmp.path())
        .env("MMCP_HOME", &mmcp_home)
        .assert()
        .success()
        .stdout(predicate::str::contains("cache rebuild complete"))
        .stdout(predicate::str::contains("1 group(s) scanned"));

    // The debug rebuild opens its own pool directly rather than
    // reusing the process-global one, so the on-disk file it wrote
    // to is exactly `default_db_path` -- assert on that path
    // directly rather than re-deriving it, so a future change to
    // the layout breaks this test instead of silently drifting.
    let db_path = mmcp_home.join("cache").join("index.sqlite3");
    assert!(
        db_path.exists(),
        "cache-rebuild must create the cache database at {}",
        db_path.display()
    );
}
