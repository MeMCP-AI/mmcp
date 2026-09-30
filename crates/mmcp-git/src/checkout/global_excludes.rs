//! The user's global git excludes file.

use std::ffi::OsString;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use super::open_checkout::open_containing;
use crate::GitError;

/// Path of the global excludes file, relative to the user's git configuration directory.
const EXCLUDES_FILE_IN_GIT_DIR: &str = "ignore";

/// Location of the global excludes file for the repository containing `path`.
/// `core.excludesFile` when it is an absolute or `~`-prefixed path, else `$XDG_CONFIG_HOME/git/ignore`, else `~/.config/git/ignore`.
/// `None` when `path` is not inside a repository.
///
/// # Errors
/// [`GitError::OpenRepo`] when the repository cannot be opened, [`GitError::GlobalExcludesFile`] when `core.excludesFile` cannot be interpolated, [`GitError::GlobalExcludesFileUnlocated`] when no location resolves.
pub fn global_excludes_file(path: &Path) -> Result<Option<PathBuf>, GitError> {
    match open_containing(path)? {
        Some(repo) => global_excludes_file_of(&repo, &mut gix::path::env::var).map(Some),
        None => Ok(None),
    }
}

/// [`global_excludes_file`] against an already opened `repo`, reading the environment through `env_var`.
pub(super) fn global_excludes_file_of(
    repo: &gix::Repository,
    env_var: &mut dyn FnMut(&str) -> Option<OsString>,
) -> Result<PathBuf, GitError> {
    if let Some(configured) = repo.config_snapshot().trusted_path("core.excludesFile") {
        let configured = configured.map_err(|error| GitError::GlobalExcludesFile {
            source: Box::new(error),
        })?;
        if configured.is_absolute() {
            return Ok(configured.into_owned());
        }
    }
    gix::path::env::xdg_config(EXCLUDES_FILE_IN_GIT_DIR, env_var)
        .or_else(|| {
            gix::path::env::home_dir().map(|home| {
                home.join(".config")
                    .join("git")
                    .join(EXCLUDES_FILE_IN_GIT_DIR)
            })
        })
        .ok_or(GitError::GlobalExcludesFileUnlocated)
}

/// Append `pattern` as its own line to `file`, creating the file and its directory when absent.
/// `false` when a line equal to `pattern` is already there and nothing was written.
///
/// # Errors
/// [`GitError::GlobalExcludesWrite`] when the file cannot be read or written.
pub(super) fn append_pattern(file: &Path, pattern: &str) -> Result<bool, GitError> {
    let write_error = |source: std::io::Error| GitError::GlobalExcludesWrite {
        path: file.display().to_string(),
        source,
    };
    let existing = match std::fs::read_to_string(file) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(write_error(error)),
    };
    if existing.lines().any(|line| line == pattern) {
        return Ok(false);
    }
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent).map_err(write_error)?;
    }
    let separator = if existing.is_empty() || existing.ends_with('\n') {
        ""
    } else {
        "\n"
    };
    let mut handle = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(file)
        .map_err(write_error)?;
    handle
        .write_all(format!("{separator}{pattern}\n").as_bytes())
        .map_err(write_error)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    fn isolated_repo(root: &Path, config: &str) -> gix::Repository {
        gix::init(root).unwrap();
        if !config.is_empty() {
            std::fs::write(root.join(".git").join("config"), config).unwrap();
        }
        gix::open_opts(root, gix::open::Options::isolated()).unwrap()
    }

    fn env_of(pairs: &[(&'static str, PathBuf)]) -> impl FnMut(&str) -> Option<OsString> {
        let pairs = pairs.to_vec();
        move |name| {
            pairs
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| value.clone().into_os_string())
        }
    }

    #[test]
    fn core_excludesfile_wins_when_absolute() {
        let scratch = tempfile::TempDir::new().unwrap();
        let configured = scratch.path().join("configured-ignore");
        let repo = isolated_repo(
            &scratch.path().join("repo"),
            &format!(
                "[core]\n\texcludesFile = {}\n",
                configured.display().to_string().replace('\\', "/")
            ),
        );
        let xdg = scratch.path().join("xdg");

        let resolved =
            global_excludes_file_of(&repo, &mut env_of(&[("XDG_CONFIG_HOME", xdg)])).unwrap();

        assert_eq!(resolved, configured);
    }

    #[test]
    fn xdg_config_home_is_used_when_core_excludesfile_is_unset() {
        let scratch = tempfile::TempDir::new().unwrap();
        let repo = isolated_repo(&scratch.path().join("repo"), "");
        let xdg = scratch.path().join("xdg");

        let resolved =
            global_excludes_file_of(&repo, &mut env_of(&[("XDG_CONFIG_HOME", xdg.clone())]))
                .unwrap();

        assert_eq!(resolved, xdg.join("git").join("ignore"));
    }

    #[test]
    fn home_config_is_used_when_xdg_config_home_is_unset() {
        let scratch = tempfile::TempDir::new().unwrap();
        let repo = isolated_repo(&scratch.path().join("repo"), "");
        let home = scratch.path().join("home");

        let resolved =
            global_excludes_file_of(&repo, &mut env_of(&[("HOME", home.clone())])).unwrap();

        assert_eq!(resolved, home.join(".config").join("git").join("ignore"));
    }

    #[test]
    fn a_relative_core_excludesfile_falls_back_to_the_xdg_location() {
        let scratch = tempfile::TempDir::new().unwrap();
        let repo = isolated_repo(
            &scratch.path().join("repo"),
            "[core]\n\texcludesFile = relative-ignore\n",
        );
        let xdg = scratch.path().join("xdg");

        let resolved =
            global_excludes_file_of(&repo, &mut env_of(&[("XDG_CONFIG_HOME", xdg.clone())]))
                .unwrap();

        assert_eq!(resolved, xdg.join("git").join("ignore"));
    }

    #[test]
    fn append_creates_the_file_and_its_directory_and_writes_one_line() {
        let scratch = tempfile::TempDir::new().unwrap();
        let file = scratch.path().join("git").join("ignore");

        assert!(append_pattern(&file, "**/.mmcp.local.toml").unwrap());

        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "**/.mmcp.local.toml\n"
        );
    }

    #[test]
    fn append_adds_the_pattern_once() {
        let scratch = tempfile::TempDir::new().unwrap();
        let file = scratch.path().join("ignore");

        assert!(append_pattern(&file, "**/.mmcp.local.toml").unwrap());
        assert!(!append_pattern(&file, "**/.mmcp.local.toml").unwrap());

        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "**/.mmcp.local.toml\n"
        );
    }

    #[test]
    fn append_keeps_existing_lines_and_starts_a_new_line() {
        let scratch = tempfile::TempDir::new().unwrap();
        let file = scratch.path().join("ignore");
        std::fs::write(&file, "*.log").unwrap();

        assert!(append_pattern(&file, "**/.mmcp.local.toml").unwrap());

        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "*.log\n**/.mmcp.local.toml\n"
        );
    }

    #[test]
    fn a_path_outside_any_repository_has_no_global_excludes_file() {
        let scratch = tempfile::TempDir::new().unwrap();

        assert_eq!(global_excludes_file(scratch.path()).unwrap(), None);
    }
}
