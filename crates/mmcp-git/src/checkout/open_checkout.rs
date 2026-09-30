//! Opening the repository a path lives in.

use std::path::{Path, PathBuf};

use crate::GitError;

/// Deepest existing ancestor of `path`, `path` itself when it exists.
fn existing_ancestor(path: &Path) -> &Path {
    path.ancestors()
        .find(|ancestor| ancestor.exists())
        .unwrap_or(path)
}

/// Open the repository containing `path`, searching upwards from its deepest existing ancestor.
/// `None` when no repository contains `path`.
///
/// # Errors
/// [`GitError::OpenRepo`] when a repository is found and cannot be opened.
pub(super) fn open_containing(path: &Path) -> Result<Option<gix::Repository>, GitError> {
    let start = existing_ancestor(path);
    let start = if start.is_dir() {
        start
    } else {
        start.parent().unwrap_or(start)
    };
    match gix::discover(start) {
        Ok(repo) => Ok(Some(repo)),
        Err(gix::discover::Error::Discover(gix::discover::upwards::Error::NoGitRepository {
            ..
        })) => Ok(None),
        Err(error) => Err(GitError::OpenRepo {
            path: path.display().to_string(),
            source: Box::new(error),
        }),
    }
}

/// `path` with symlinks resolved, the part of it that does not exist yet appended unchanged.
///
/// # Errors
/// [`GitError::ResolvePath`] when its deepest existing ancestor cannot be resolved.
pub(super) fn real_path(path: &Path) -> Result<PathBuf, GitError> {
    let existing = existing_ancestor(path);
    let resolved = gix::path::realpath(existing).map_err(|error| GitError::ResolvePath {
        path: existing.display().to_string(),
        source: Box::new(error),
    })?;
    Ok(match path.strip_prefix(existing) {
        Ok(missing) => resolved.join(missing),
        Err(_) => resolved,
    })
}
