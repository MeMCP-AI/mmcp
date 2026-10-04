#![allow(clippy::unwrap_used, clippy::expect_used)]
//! `mmcp config` and the `mmcp serve` notice flags, driven through the compiled binary.

use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::TempDir;

fn mmcp() -> Command {
    Command::cargo_bin("mmcp").expect("mmcp binary")
}

/// `mmcp` run in `dir`, with every home it could reach pointed inside `dir`.
/// A local write may append to the user's global git excludes file, so neither the real home nor the real git configuration may be reachable.
fn mmcp_in(dir: &std::path::Path) -> Command {
    let mut command = mmcp();
    command
        .current_dir(dir)
        .env("MMCP_HOME", dir.join("mmcp-home"))
        .env("HOME", dir.join("home"))
        .env("USERPROFILE", dir.join("home"))
        .env("XDG_CONFIG_HOME", dir.join("xdg"))
        .env("GIT_CONFIG_GLOBAL", dir.join("global-gitconfig"))
        .env("GIT_CONFIG_NOSYSTEM", "1");
    command
}

/// A scratch home and a project initialized in it, outside any git repository.
struct Project {
    tmp: TempDir,
}

impl Project {
    fn new() -> Self {
        let tmp = TempDir::new().unwrap();
        let project = Self { tmp };
        project
            .mmcp()
            .args(["init", "project", "--slug", "config-cli", "--config-only"])
            .assert()
            .success();
        project
    }

    fn mmcp(&self) -> Command {
        mmcp_in(self.tmp.path())
    }
}

#[test]
fn set_then_get_at_the_project_scope_prints_the_layers_and_the_effective_value() {
    let project = Project::new();
    let file = project.tmp.path().join(".mmcp.toml");

    project
        .mmcp()
        .args([
            "config",
            "set",
            "notice.md.project",
            "off",
            "--scope",
            "project",
        ])
        .assert()
        .success()
        .stdout(format!(
            "notice.md.project = off at project in {}.\n",
            file.display()
        ));

    project
        .mmcp()
        .args(["config", "get", "notice.md.project"])
        .assert()
        .success()
        .stdout("local: unset\nproject: off\nuser: unset\neffective: off (project)\n");
}

#[test]
fn the_local_scope_writes_its_own_file_and_leaves_the_project_file_alone() {
    let project = Project::new();
    let project_toml = std::fs::read_to_string(project.tmp.path().join(".mmcp.toml")).unwrap();

    project
        .mmcp()
        .args(["config", "set", "notice.md.user", "off", "--scope", "local"])
        .assert()
        .success();

    assert_eq!(
        std::fs::read_to_string(project.tmp.path().join(".mmcp.toml")).unwrap(),
        project_toml
    );
    let local = std::fs::read_to_string(project.tmp.path().join(".mmcp.local.toml")).unwrap();
    assert!(local.contains("user = \"off\""), "{local}");
}

#[test]
fn unset_removes_the_key_and_a_second_unset_says_it_was_not_set() {
    let project = Project::new();
    project
        .mmcp()
        .args(["config", "set", "notice.md.user", "off", "--scope", "user"])
        .assert()
        .success();

    project
        .mmcp()
        .args(["config", "unset", "notice.md.user", "--scope", "user"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "notice.md.user removed from user in",
        ));
    project
        .mmcp()
        .args(["config", "unset", "notice.md.user", "--scope", "user"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "notice.md.user not set at user in",
        ));
}

#[test]
fn a_project_scope_outside_a_project_names_the_directory_and_the_scope() {
    let tmp = TempDir::new().unwrap();
    mmcp_in(tmp.path())
        .args([
            "config",
            "set",
            "notice.md.project",
            "off",
            "--scope",
            "project",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("No mmcp project at"))
        .stderr(predicate::str::contains(
            "Scope project needs a .mmcp.toml.",
        ));
}

#[test]
fn a_set_without_a_scope_is_refused_by_the_argument_parser() {
    mmcp()
        .args(["config", "set", "notice.md.project", "off"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("--scope"));
}

#[test]
fn config_help_is_the_approved_text() {
    mmcp()
        .args(["config", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Read or change an mmcp setting. Mirrors the config MCP tool",
        ))
        .stdout(predicate::str::contains(
            "Show the local, project and user layers of a setting and its effective value",
        ))
        .stdout(predicate::str::contains("Set a setting at a scope"))
        .stdout(predicate::str::contains("Remove a setting from a scope"));
}

#[test]
fn serve_help_names_each_notice_flag_its_variable_and_what_outranks_it() {
    mmcp()
        .args(["serve", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--notice-md-project"))
        .stdout(predicate::str::contains("MMCP_NOTICE_MD_PROJECT"))
        .stdout(predicate::str::contains("--notice-md-user"))
        .stdout(predicate::str::contains("MMCP_NOTICE_MD_USER"))
        .stdout(predicate::str::contains(
            "Show or hide the project CLAUDE.md block notices for this run, unless a local or project setting decides",
        ))
        .stdout(predicate::str::contains(
            "Show or hide the ~/.claude/CLAUDE.md block notice for this run, unless a local or project setting decides",
        ));
}

#[test]
fn a_notice_variable_that_is_not_on_or_off_stops_serve_before_it_starts() {
    mmcp()
        .arg("serve")
        .env("MMCP_NOTICE_MD_PROJECT", "maybe")
        .assert()
        .failure()
        .stderr(predicate::str::contains("--notice-md-project"))
        .stderr(predicate::str::contains("maybe"));
}
