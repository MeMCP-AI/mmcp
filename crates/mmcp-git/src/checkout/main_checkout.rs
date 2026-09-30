//! Locating a path inside the main checkout of a linked worktree.

use std::path::{Path, PathBuf};

use super::open_checkout::{open_containing, real_path};
use crate::GitError;

/// For a `path` inside a linked git worktree, the same relative path under the main checkout's root.
/// `None` when `path` is in the main checkout itself, in a repository without a linked worktree, or outside any repository.
///
/// # Errors
/// [`GitError::OpenRepo`] when the repository or its main checkout cannot be opened.
pub fn main_checkout_counterpart(path: &Path) -> Result<Option<PathBuf>, GitError> {
    let Some(repo) = open_containing(path)? else {
        return Ok(None);
    };
    if repo.kind() != gix::repository::Kind::LinkedWorkTree {
        return Ok(None);
    }
    let Some(worktree_root) = repo.workdir() else {
        return Ok(None);
    };
    let main_repo = repo.main_repo().map_err(|error| GitError::OpenRepo {
        path: repo.common_dir().display().to_string(),
        source: Box::new(error),
    })?;
    let Some(main_root) = main_repo.workdir() else {
        return Ok(None);
    };
    let real_target = real_path(path)?;
    let real_worktree_root = real_path(worktree_root)?;
    let real_main_root = real_path(main_root)?;
    Ok(real_target
        .strip_prefix(&real_worktree_root)
        .ok()
        .map(|relative| real_main_root.join(relative)))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use crate::checkout::LinkedWorktreeFixture;

    #[test]
    fn a_path_in_a_linked_worktree_maps_to_the_same_relative_path_under_the_main_checkout() {
        let fixture = LinkedWorktreeFixture::new().unwrap();
        let project = fixture.linked.join("crates").join("inner");
        std::fs::create_dir_all(&project).unwrap();

        let counterpart = main_checkout_counterpart(&project).unwrap();

        assert_eq!(counterpart, Some(fixture.main.join("crates").join("inner")));
    }

    #[test]
    fn the_root_of_a_linked_worktree_maps_to_the_root_of_the_main_checkout() {
        let fixture = LinkedWorktreeFixture::new().unwrap();

        let counterpart = main_checkout_counterpart(&fixture.linked).unwrap();

        assert_eq!(counterpart, Some(fixture.main.clone()));
    }

    #[test]
    fn a_path_in_the_main_checkout_itself_has_no_counterpart() {
        let fixture = LinkedWorktreeFixture::new().unwrap();
        let inner = fixture.main.join("crates");
        std::fs::create_dir_all(&inner).unwrap();

        assert_eq!(main_checkout_counterpart(&inner).unwrap(), None);
    }

    #[test]
    fn a_path_outside_any_repository_has_no_counterpart() {
        let scratch = tempfile::TempDir::new().unwrap();

        assert_eq!(main_checkout_counterpart(scratch.path()).unwrap(), None);
    }
}
