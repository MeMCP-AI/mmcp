//! Keeping a personal file out of git through the global excludes file.

use std::ffi::OsString;
use std::path::Path;

use super::Exclusion;
use super::global_excludes::{append_pattern, global_excludes_file_of};
use super::open_checkout::open_containing;
use super::path_exclusion::is_excluded_in;
use super::path_tracking::is_tracked_in;
use crate::GitError;

/// Make sure the repository containing `path` ignores it, appending `pattern` to the user's global excludes file when it does not.
/// Nothing is written when the repository's ignore rules already exclude `path`, when `path` is outside a repository, or when the repository tracks `path`, which no ignore rule can undo.
///
/// # Errors
/// [`GitError::OpenRepo`], [`GitError::IgnoreRules`], [`GitError::GlobalExcludesFile`], [`GitError::GlobalExcludesFileUnlocated`] and [`GitError::GlobalExcludesWrite`], each naming its own step.
pub fn exclude_path_globally(path: &Path, pattern: &str) -> Result<Exclusion, GitError> {
    match open_containing(path)? {
        Some(repo) => exclude_path_in(&repo, path, pattern, &mut gix::path::env::var),
        None => Ok(Exclusion::NotInRepository),
    }
}

/// [`exclude_path_globally`] against an already opened `repo`, reading the environment through `env_var`.
pub(super) fn exclude_path_in(
    repo: &gix::Repository,
    path: &Path,
    pattern: &str,
    env_var: &mut dyn FnMut(&str) -> Option<OsString>,
) -> Result<Exclusion, GitError> {
    if is_tracked_in(repo, path)? == Some(true) {
        return Ok(Exclusion::Tracked);
    }
    match is_excluded_in(repo, path)? {
        None => Ok(Exclusion::NotInRepository),
        Some(true) => Ok(Exclusion::AlreadyExcluded),
        Some(false) => {
            let file = global_excludes_file_of(repo, env_var)?;
            if append_pattern(&file, pattern)? {
                Ok(Exclusion::Appended { file })
            } else {
                Ok(Exclusion::AlreadyExcluded)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use std::path::PathBuf;

    use super::super::path_tracking::track_in_index;
    use super::*;

    const PATTERN: &str = "**/.mmcp.local.toml";

    fn isolated_repo(root: &Path) -> gix::Repository {
        gix::init(root).unwrap();
        gix::open_opts(root, gix::open::Options::isolated()).unwrap()
    }

    fn xdg_env(xdg: &Path) -> impl FnMut(&str) -> Option<OsString> {
        let xdg: PathBuf = xdg.to_path_buf();
        move |name| (name == "XDG_CONFIG_HOME").then(|| xdg.clone().into_os_string())
    }

    #[test]
    fn a_first_write_appends_the_pattern_to_the_global_excludes_file_and_reports_it() {
        let scratch = tempfile::TempDir::new().unwrap();
        let repo_root = scratch.path().join("repo");
        let repo = isolated_repo(&repo_root);
        let xdg = scratch.path().join("xdg");

        let outcome = exclude_path_in(
            &repo,
            &repo_root.join(".mmcp.local.toml"),
            PATTERN,
            &mut xdg_env(&xdg),
        )
        .unwrap();

        let expected = xdg.join("git").join("ignore");
        assert_eq!(
            outcome,
            Exclusion::Appended {
                file: expected.clone()
            }
        );
        assert_eq!(
            std::fs::read_to_string(expected).unwrap(),
            format!("{PATTERN}\n")
        );
    }

    #[test]
    fn a_repeat_write_appends_nothing_because_the_pattern_now_excludes_the_file() {
        let scratch = tempfile::TempDir::new().unwrap();
        let repo_root = scratch.path().join("repo");
        let xdg = scratch.path().join("xdg");
        let ignore = xdg.join("git").join("ignore");
        std::fs::create_dir_all(ignore.parent().unwrap()).unwrap();
        std::fs::write(&ignore, format!("{PATTERN}\n")).unwrap();
        let repo = isolated_repo(&repo_root);
        // The isolated repository reads no environment, so point core.excludesFile at the same file.
        std::fs::write(
            repo_root.join(".git").join("config"),
            format!(
                "[core]\n\texcludesFile = {}\n",
                ignore.display().to_string().replace('\\', "/")
            ),
        )
        .unwrap();
        let repo = gix::open_opts(repo.git_dir(), gix::open::Options::isolated()).unwrap();

        let outcome = exclude_path_in(
            &repo,
            &repo_root.join(".mmcp.local.toml"),
            PATTERN,
            &mut xdg_env(&xdg),
        )
        .unwrap();

        assert_eq!(outcome, Exclusion::AlreadyExcluded);
        assert_eq!(
            std::fs::read_to_string(ignore).unwrap(),
            format!("{PATTERN}\n")
        );
    }

    #[test]
    fn a_local_write_where_the_repository_gitignore_already_excludes_the_file_appends_nothing() {
        let scratch = tempfile::TempDir::new().unwrap();
        let repo_root = scratch.path().join("repo");
        let repo = isolated_repo(&repo_root);
        std::fs::write(repo_root.join(".gitignore"), ".mmcp.local.toml\n").unwrap();
        let xdg = scratch.path().join("xdg");

        let outcome = exclude_path_in(
            &repo,
            &repo_root.join(".mmcp.local.toml"),
            PATTERN,
            &mut xdg_env(&xdg),
        )
        .unwrap();

        assert_eq!(outcome, Exclusion::AlreadyExcluded);
        assert!(!xdg.exists(), "no global excludes file is created");
    }

    #[test]
    fn a_tracked_file_is_reported_tracked_and_nothing_is_appended() {
        let scratch = tempfile::TempDir::new().unwrap();
        let repo_root = scratch.path().join("repo");
        let repo = isolated_repo(&repo_root);
        track_in_index(&repo, ".mmcp.local.toml");
        let xdg = scratch.path().join("xdg");

        let outcome = exclude_path_in(
            &repo,
            &repo_root.join(".mmcp.local.toml"),
            PATTERN,
            &mut xdg_env(&xdg),
        )
        .unwrap();

        assert_eq!(outcome, Exclusion::Tracked);
        assert!(!xdg.exists(), "no global excludes file is created");
    }

    #[test]
    fn a_tracked_file_that_an_ignore_rule_matches_is_still_reported_tracked() {
        let scratch = tempfile::TempDir::new().unwrap();
        let repo_root = scratch.path().join("repo");
        let repo = isolated_repo(&repo_root);
        std::fs::write(repo_root.join(".gitignore"), ".mmcp.local.toml\n").unwrap();
        track_in_index(&repo, ".mmcp.local.toml");
        let xdg = scratch.path().join("xdg");

        let outcome = exclude_path_in(
            &repo,
            &repo_root.join(".mmcp.local.toml"),
            PATTERN,
            &mut xdg_env(&xdg),
        )
        .unwrap();

        assert_eq!(outcome, Exclusion::Tracked);
    }

    #[test]
    fn a_path_outside_any_repository_reports_not_in_repository_and_writes_nothing() {
        let scratch = tempfile::TempDir::new().unwrap();

        let outcome =
            exclude_path_globally(&scratch.path().join(".mmcp.local.toml"), PATTERN).unwrap();

        assert_eq!(outcome, Exclusion::NotInRepository);
    }
}
