//! Whether a repository tracks a path.

use std::path::Path;

use super::path_exclusion::relative_to_workdir;
use crate::GitError;

/// Whether the index of `repo` holds `path`, which makes it tracked whatever the ignore rules say.
/// `None` when `path` is not inside the work tree of `repo`.
///
/// # Errors
/// [`GitError::IgnoreRules`] when the index cannot be read.
pub(super) fn is_tracked_in(repo: &gix::Repository, path: &Path) -> Result<Option<bool>, GitError> {
    let Some(relative) = relative_to_workdir(repo, path)? else {
        return Ok(None);
    };
    let index = repo
        .index_or_empty()
        .map_err(|error| GitError::IgnoreRules {
            path: path.display().to_string(),
            source: Box::new(error),
        })?;
    let unix_relative = gix::path::to_unix_separators_on_windows(gix::path::into_bstr(relative));
    Ok(Some(index.entry_by_path(&unix_relative).is_some()))
}

/// Record `relative` in the index of `repo` as a regular file.
#[cfg(test)]
#[allow(clippy::unwrap_used)]
pub(super) fn track_in_index(repo: &gix::Repository, relative: &str) {
    let mut state = gix::index::State::new(repo.object_hash());
    state.dangerously_push_entry(
        gix::index::entry::Stat::default(),
        repo.object_hash().null(),
        gix::index::entry::Flags::empty(),
        gix::index::entry::Mode::FILE,
        relative.into(),
    );
    state.sort_entries();
    let mut file = gix::index::File::from_state(state, repo.index_path());
    file.write(gix::index::write::Options::default()).unwrap();
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    const LOCAL_FILE: &str = ".mmcp.local.toml";

    fn isolated_repo(root: &Path) -> gix::Repository {
        gix::init(root).unwrap();
        gix::open_opts(root, gix::open::Options::isolated()).unwrap()
    }

    #[test]
    fn a_path_in_the_index_is_tracked() {
        let scratch = tempfile::TempDir::new().unwrap();
        let repo = isolated_repo(scratch.path());
        track_in_index(&repo, LOCAL_FILE);

        assert_eq!(
            is_tracked_in(&repo, &scratch.path().join(LOCAL_FILE)).unwrap(),
            Some(true)
        );
    }

    #[test]
    fn a_nested_path_in_the_index_is_tracked_under_its_unix_spelling() {
        let scratch = tempfile::TempDir::new().unwrap();
        let repo = isolated_repo(scratch.path());
        let nested = scratch.path().join("crates").join("inner");
        std::fs::create_dir_all(&nested).unwrap();
        track_in_index(&repo, &format!("crates/inner/{LOCAL_FILE}"));

        assert_eq!(
            is_tracked_in(&repo, &nested.join(LOCAL_FILE)).unwrap(),
            Some(true)
        );
    }

    #[test]
    fn a_path_missing_from_the_index_is_not_tracked() {
        let scratch = tempfile::TempDir::new().unwrap();
        let repo = isolated_repo(scratch.path());
        track_in_index(&repo, "other.toml");

        assert_eq!(
            is_tracked_in(&repo, &scratch.path().join(LOCAL_FILE)).unwrap(),
            Some(false)
        );
    }

    #[test]
    fn a_repository_without_an_index_tracks_nothing() {
        let scratch = tempfile::TempDir::new().unwrap();
        let repo = isolated_repo(scratch.path());

        assert_eq!(
            is_tracked_in(&repo, &scratch.path().join(LOCAL_FILE)).unwrap(),
            Some(false)
        );
    }

    #[test]
    fn a_path_outside_the_work_tree_has_no_answer() {
        let scratch = tempfile::TempDir::new().unwrap();
        let repo = isolated_repo(&scratch.path().join("repo"));
        let elsewhere = tempfile::TempDir::new().unwrap();

        assert_eq!(
            is_tracked_in(&repo, &elsewhere.path().join(LOCAL_FILE)).unwrap(),
            None
        );
    }
}
