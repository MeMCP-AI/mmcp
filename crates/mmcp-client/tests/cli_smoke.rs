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

    let config_path = tmp.path().join(".mmcp").join("config.toml");
    assert!(config_path.exists(), "init should create .mmcp/config.toml");
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
