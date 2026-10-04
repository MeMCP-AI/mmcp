//! Whether a repository's ignore rules exclude a path.

use std::path::{Path, PathBuf};

use gix::worktree::stack::state::ignore::Source;

use super::open_checkout::{open_containing, own_path, read_index, real_path};
use crate::GitError;

/// Whether the ignore rules of the repository containing `path` exclude it.
/// The rules are every `.gitignore` of the work tree, `$GIT_COMMON_DIR/info/exclude` and the global excludes file.
/// `None` when `path` is not inside a repository's work tree.
///
/// # Errors
/// [`GitError::OpenRepo`] when the repository cannot be opened, [`GitError::IgnoreRules`] when its rules cannot be evaluated.
pub fn is_path_excluded(path: &Path) -> Result<Option<bool>, GitError> {
    match open_containing(path)? {
        Some(repo) => is_excluded_in(&repo, path),
        None => Ok(None),
    }
}

/// The path of `path` relative to the work tree of `repo`, `None` when `path` is not inside it.
/// A symlink is named by its own path, so a link pointing outside the work tree is still inside it.
pub(super) fn relative_to_workdir(
    repo: &gix::Repository,
    path: &Path,
) -> Result<Option<PathBuf>, GitError> {
    let Some(workdir) = repo.workdir() else {
        return Ok(None);
    };
    let real_workdir = real_path(workdir)?;
    let own_target = own_path(path)?;
    Ok(own_target
        .strip_prefix(&real_workdir)
        .ok()
        .map(Path::to_path_buf))
}

/// [`is_path_excluded`] against an already opened `repo`.
pub(super) fn is_excluded_in(
    repo: &gix::Repository,
    path: &Path,
) -> Result<Option<bool>, GitError> {
    let Some(relative) = relative_to_workdir(repo, path)? else {
        return Ok(None);
    };
    let relative = relative.as_path();
    let ignore_error = |source: Box<dyn std::error::Error + Send + Sync>| GitError::IgnoreRules {
        path: path.display().to_string(),
        source,
    };
    let index = read_index(repo, path)?;
    let mut stack = repo
        .excludes(&index, None, Source::WorktreeThenIdMappingIfNotSkipped)
        .map_err(|error| ignore_error(Box::new(error)))?;
    let platform = stack
        .at_path(relative, None)
        .map_err(|error| ignore_error(Box::new(error)))?;
    Ok(Some(platform.is_excluded()))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::super::open_checkout::unreadable_index;
    use super::*;

    const LOCAL_FILE: &str = ".mmcp.local.toml";

    /// A repository opened without the user's global git configuration or environment.
    pub(super) fn isolated_repo(root: &Path) -> gix::Repository {
        gix::init(root).expect("init repository");
        gix::open_opts(root, gix::open::Options::isolated()).expect("open isolated")
    }

    #[test]
    fn a_path_in_a_repository_without_a_matching_rule_is_not_excluded() {
        let scratch = tempfile::TempDir::new().unwrap();
        let repo = isolated_repo(scratch.path());

        assert_eq!(
            is_excluded_in(&repo, &scratch.path().join(LOCAL_FILE)).unwrap(),
            Some(false)
        );
    }

    #[test]
    fn an_unreadable_index_is_a_read_index_error_and_not_an_ignore_rules_error() {
        let scratch = tempfile::TempDir::new().unwrap();
        let repo = isolated_repo(scratch.path());
        std::fs::write(repo.index_path(), unreadable_index()).unwrap();

        let error = is_excluded_in(&repo, &scratch.path().join(LOCAL_FILE)).unwrap_err();

        assert!(matches!(error, GitError::ReadIndex { .. }), "{error:?}");
    }

    #[test]
    fn the_repository_gitignore_excludes_the_path() {
        let scratch = tempfile::TempDir::new().unwrap();
        let repo = isolated_repo(scratch.path());
        std::fs::write(scratch.path().join(".gitignore"), format!("{LOCAL_FILE}\n")).unwrap();

        assert_eq!(
            is_excluded_in(&repo, &scratch.path().join(LOCAL_FILE)).unwrap(),
            Some(true)
        );
    }

    #[test]
    fn a_nested_gitignore_excludes_a_path_below_it() {
        let scratch = tempfile::TempDir::new().unwrap();
        let repo = isolated_repo(scratch.path());
        let nested = scratch.path().join("crates").join("inner");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(nested.join(".gitignore"), "*.local.toml\n").unwrap();

        assert_eq!(
            is_excluded_in(&repo, &nested.join(LOCAL_FILE)).unwrap(),
            Some(true)
        );
        assert_eq!(
            is_excluded_in(&repo, &scratch.path().join(LOCAL_FILE)).unwrap(),
            Some(false)
        );
    }

    #[test]
    fn info_exclude_excludes_the_path() {
        let scratch = tempfile::TempDir::new().unwrap();
        let repo = isolated_repo(scratch.path());
        let info = repo.common_dir().join("info");
        std::fs::create_dir_all(&info).unwrap();
        std::fs::write(info.join("exclude"), format!("{LOCAL_FILE}\n")).unwrap();

        assert_eq!(
            is_excluded_in(&repo, &scratch.path().join(LOCAL_FILE)).unwrap(),
            Some(true)
        );
    }

    #[test]
    fn the_global_excludes_file_named_by_core_excludesfile_excludes_the_path() {
        let scratch = tempfile::TempDir::new().unwrap();
        let excludes = scratch.path().join("global-ignore");
        std::fs::write(&excludes, format!("**/{LOCAL_FILE}\n")).unwrap();
        let repo_root = scratch.path().join("repo");
        std::fs::create_dir_all(&repo_root).unwrap();
        gix::init(&repo_root).unwrap();
        std::fs::write(
            repo_root.join(".git").join("config"),
            format!(
                "[core]\n\texcludesFile = {}\n",
                excludes.display().to_string().replace('\\', "/")
            ),
        )
        .unwrap();
        let repo = gix::open_opts(&repo_root, gix::open::Options::isolated()).unwrap();

        assert_eq!(
            is_excluded_in(&repo, &repo_root.join(LOCAL_FILE)).unwrap(),
            Some(true)
        );
    }

    #[test]
    fn a_path_outside_any_repository_has_no_answer() {
        let scratch = tempfile::TempDir::new().unwrap();

        assert_eq!(
            is_path_excluded(&scratch.path().join(LOCAL_FILE)).unwrap(),
            None
        );
    }
}
