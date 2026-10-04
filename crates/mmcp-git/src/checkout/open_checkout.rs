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

/// `path` with the symlinks of its directories resolved and its own last component kept.
/// Git names a symlink by its own path, whatever it points at.
///
/// # Errors
/// [`GitError::ResolvePath`] when the deepest existing ancestor of its directory cannot be resolved.
pub(super) fn own_path(path: &Path) -> Result<PathBuf, GitError> {
    match (path.parent(), path.file_name()) {
        (Some(parent), Some(name)) => Ok(real_path(parent)?.join(name)),
        _ => real_path(path),
    }
}

/// Bytes of an index file that carry the signature but an unsupported version, for the tests of the read failure.
#[cfg(test)]
pub(super) fn unreadable_index() -> Vec<u8> {
    const SIGNATURE: &[u8] = b"DIRC";
    const UNSUPPORTED_VERSION: [u8; 4] = [0, 0, 0, 99];
    const BODY_BYTES: usize = 64;
    let mut bytes = SIGNATURE.to_vec();
    bytes.extend_from_slice(&UNSUPPORTED_VERSION);
    bytes.resize(bytes.len() + BODY_BYTES, 0);
    bytes
}

/// The index of `repo`, empty when it has none yet.
///
/// # Errors
/// [`GitError::ReadIndex`] when the index file exists and cannot be read.
pub(super) fn read_index(
    repo: &gix::Repository,
    path: &Path,
) -> Result<gix::worktree::Index, GitError> {
    repo.index_or_empty().map_err(|error| GitError::ReadIndex {
        path: path.display().to_string(),
        source: Box::new(error),
    })
}
