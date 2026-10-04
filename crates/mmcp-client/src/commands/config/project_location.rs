//! [`ProjectLocation`], the project a `config` operation addresses.

use std::path::{Path, PathBuf};

use mmcp_core::config::ConfigScope;
use mmcp_store::StoreError;
use mmcp_store::config::resolve_project_root;

use super::ConfigOpError;

/// Where a `config` operation looked for its project, and the root it found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectLocation {
    searched_from: PathBuf,
    root: Option<PathBuf>,
}

impl ProjectLocation {
    /// Look for the project: `explicit` must itself carry `.mmcp.toml`, otherwise `working_directory` and its ancestors are walked.
    /// A search that finds no project is a location without a root.
    ///
    /// # Errors
    /// [`ConfigOpError::ResolveProject`] when the search fails for a reason other than finding no project.
    pub fn find(explicit: Option<&Path>, working_directory: &Path) -> Result<Self, ConfigOpError> {
        let searched_from = explicit.unwrap_or(working_directory).to_path_buf();
        let root = match resolve_project_root(explicit, Some(working_directory)) {
            Ok(root) => Some(root),
            Err(StoreError::ProjectRootNotFound) => None,
            Err(other) => return Err(ConfigOpError::ResolveProject(other)),
        };
        Ok(Self {
            searched_from,
            root,
        })
    }

    /// A location that found `root`, searched from the root itself.
    #[cfg(test)]
    #[must_use]
    pub fn at(root: PathBuf) -> Self {
        Self {
            searched_from: root.clone(),
            root: Some(root),
        }
    }

    /// A location that found no project after searching from `searched_from`.
    #[cfg(test)]
    #[must_use]
    pub fn none(searched_from: PathBuf) -> Self {
        Self {
            searched_from,
            root: None,
        }
    }

    /// The project root, `None` when the search found none.
    #[must_use]
    pub fn root(&self) -> Option<&Path> {
        self.root.as_deref()
    }

    /// The project root a project-file scope needs.
    ///
    /// # Errors
    /// [`ConfigOpError::ProjectRootRequired`] naming the directory searched from.
    pub fn require_root(&self, scope: ConfigScope) -> Result<&Path, ConfigOpError> {
        self.root()
            .ok_or_else(|| ConfigOpError::ProjectRootRequired {
                scope,
                searched_from: self.searched_from.clone(),
            })
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::super::config_fixture::ConfigFixture;
    use super::*;

    #[test]
    fn an_explicit_root_carrying_the_manifest_is_the_root() {
        let fixture = ConfigFixture::new();
        let elsewhere = fixture.project.join("..");

        let location = ProjectLocation::find(Some(&fixture.project), &elsewhere).unwrap();

        assert_eq!(location.root(), Some(fixture.project.as_path()));
    }

    #[test]
    fn without_an_explicit_root_the_working_directory_is_walked_upward() {
        let fixture = ConfigFixture::new();
        let nested = fixture.project.join("crates").join("inner");
        std::fs::create_dir_all(&nested).unwrap();

        let location = ProjectLocation::find(None, &nested).unwrap();

        assert_eq!(location.root(), Some(fixture.project.as_path()));
    }

    #[test]
    fn an_explicit_directory_without_the_manifest_finds_no_project() {
        let fixture = ConfigFixture::new();
        let bare = fixture.project.join("bare");
        std::fs::create_dir_all(&bare).unwrap();

        let location = ProjectLocation::find(Some(&bare), &fixture.project).unwrap();

        assert_eq!(location.root(), None);
        let error = location.require_root(ConfigScope::Local).unwrap_err();
        assert!(
            matches!(
                &error,
                ConfigOpError::ProjectRootRequired { scope: ConfigScope::Local, searched_from }
                    if *searched_from == bare
            ),
            "the error names the explicit directory, not the working directory: {error:?}"
        );
    }

    #[test]
    fn a_missing_project_names_the_working_directory_when_none_was_given() {
        let elsewhere = tempfile::TempDir::new().unwrap();
        let outside = elsewhere.path().join("outside");
        std::fs::create_dir_all(&outside).unwrap();

        let location = ProjectLocation::find(None, &outside).unwrap();
        let error = location.require_root(ConfigScope::Project).unwrap_err();

        assert!(
            matches!(
                &error,
                ConfigOpError::ProjectRootRequired { searched_from, .. } if *searched_from == outside
            ),
            "{error:?}"
        );
    }
}
