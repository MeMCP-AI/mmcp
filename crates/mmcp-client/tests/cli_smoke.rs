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
fn init_in_tempdir_creates_config() {
    let tmp = tempfile::tempdir().unwrap();
    mmcp()
        .arg("init")
        .current_dir(tmp.path())
        .assert()
        .success();

    let config_path = tmp.path().join(".mmcp.toml");
    assert!(config_path.exists(), "init should create .mmcp.toml");
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
    // Bootstrap the project first, then probe status from the same
    // directory.
    mmcp()
        .arg("init")
        .current_dir(tmp.path())
        .assert()
        .success();

    mmcp()
        .arg("status")
        .current_dir(tmp.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("project root"))
        .stdout(predicate::str::contains("project uuid"))
        .stdout(predicate::str::contains("server"))
        .stdout(predicate::str::contains("default group"));
}

#[test]
fn init_refuses_to_overwrite_existing_project() {
    let tmp = tempfile::tempdir().unwrap();
    mmcp().arg("init").current_dir(tmp.path()).assert().success();
    mmcp()
        .arg("init")
        .current_dir(tmp.path())
        .assert()
        .failure()
        .stderr(predicate::str::contains("already initialized"));
}

#[test]
fn sync_fails_when_project_has_no_sync_block() {
    // `mmcp init` writes a project config with `sync = None`, so
    // running `mmcp sync` against a fresh init must fail with a
    // message naming the missing block.
    let tmp = tempfile::tempdir().unwrap();
    mmcp().arg("init").current_dir(tmp.path()).assert().success();

    // Point MMCP_HOME at the tempdir so the command never touches
    // the operator's real `~/.mmcp/`.
    let mmcp_home = tmp.path().join("mmcp-home");
    mmcp()
        .arg("sync")
        .current_dir(tmp.path())
        .env("MMCP_HOME", &mmcp_home)
        .assert()
        .failure()
        .stderr(predicate::str::contains("no [sync]"));
}

#[test]
fn sync_outside_project_fails_with_a_useful_message() {
    let tmp = tempfile::tempdir().unwrap();
    let mmcp_home = tmp.path().join("mmcp-home");
    mmcp()
        .arg("sync")
        .current_dir(tmp.path())
        .env("MMCP_HOME", &mmcp_home)
        .assert()
        .failure()
        .stderr(predicate::str::contains("no mmcp project"));
}

#[test]
fn hook_user_prompt_emits_session_marker() {
    // Drive the hook binary end-to-end: feed it a minimal
    // `UserPromptSubmit` JSON payload via stdin and assert the
    // standard session marker lands on stdout. Keeping the state
    // under a tempdir-backed `MMCP_HOME` keeps the test off the
    // operator's real `~/.mmcp/sessions/`.
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
    // `mmcp init claude --override --dry-run` on a missing CLAUDE.md
    // must print its plan to stderr and leave the filesystem
    // unchanged. Covers the dry-run path of the new subcommand the
    // previous track added.
    let tmp = tempfile::tempdir().unwrap();
    mmcp().arg("init").current_dir(tmp.path()).assert().success();

    mmcp()
        .args(["init", "claude", "--override", "--dry-run"])
        .current_dir(tmp.path())
        .assert()
        .success()
        .stderr(predicate::str::contains("dry-run"));

    assert!(
        !tmp.path().join("CLAUDE.md").exists(),
        "dry-run must not write the file"
    );
}
