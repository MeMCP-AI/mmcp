//! [`ConfigOpError`], why a `config` operation failed.

use std::path::PathBuf;

use mmcp_core::config::ConfigScope;
use mmcp_git::GitError;
use mmcp_store::StoreError;

/// Failure of a get, set or unset of a configuration key.
/// One variant per cause; the adapters (MCP tool, CLI) map each variant to their own wire form.
/// The cause stays a walkable source.
#[derive(Debug, thiserror::Error)]
pub enum ConfigOpError {
    /// The scope stores its key in a project file, and no project was found.
    #[error(
        "No mmcp project at {searched_from}. Scope {scope} needs a .mmcp.toml.",
        searched_from = searched_from.display(),
        scope = scope.as_str()
    )]
    ProjectRootRequired {
        /// The scope that needs a project.
        scope: ConfigScope,
        /// The directory the project was searched from.
        searched_from: PathBuf,
    },

    /// The project root could not be resolved for a reason other than finding no project.
    #[error("The project root could not be resolved.")]
    ResolveProject(#[source] StoreError),

    /// The user-level `~/.mmcp/config.toml` could not be loaded.
    #[error("The user config could not be loaded.")]
    LoadUser(#[source] StoreError),

    /// The project's `.mmcp.toml` could not be loaded.
    #[error("The project config could not be loaded.")]
    LoadProject(#[source] StoreError),

    /// The project's `.mmcp.local.toml` could not be located or loaded.
    #[error("The local config could not be loaded.")]
    LoadLocal(#[source] StoreError),

    /// The user-level `~/.mmcp/config.toml` could not be written.
    #[error("The user config could not be written.")]
    SaveUser(#[source] StoreError),

    /// The project's `.mmcp.toml` could not be written.
    #[error("The project config could not be written.")]
    SaveProject(#[source] StoreError),

    /// The project's `.mmcp.local.toml` could not be written.
    #[error("The local config could not be written.")]
    SaveLocal(#[source] StoreError),

    /// The local file could not be kept out of git, so it was not written.
    #[error("The local config could not be kept out of git, so it was not written.")]
    ExcludeLocal(#[source] GitError),
}

impl ConfigOpError {
    /// Stable wire code of the error.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::ProjectRootRequired { .. } => "project_not_found",
            Self::ResolveProject(_) => "project_root_resolution_failed",
            Self::LoadUser(_) => "user_config_load_failed",
            Self::LoadProject(_) => "project_config_load_failed",
            Self::LoadLocal(_) => "local_config_load_failed",
            Self::SaveUser(_) => "user_config_save_failed",
            Self::SaveProject(_) => "project_config_save_failed",
            Self::SaveLocal(_) => "local_config_save_failed",
            Self::ExcludeLocal(_) => "local_config_exclude_failed",
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use std::collections::HashSet;
    use std::error::Error;
    use std::path::Path;

    use super::super::config_fixture::fails_to_exclude;
    use super::*;

    fn every_variant() -> Vec<ConfigOpError> {
        let store = || StoreError::ProjectRootNotFound;
        vec![
            ConfigOpError::ProjectRootRequired {
                scope: ConfigScope::Local,
                searched_from: PathBuf::from("somewhere"),
            },
            ConfigOpError::ResolveProject(store()),
            ConfigOpError::LoadUser(store()),
            ConfigOpError::LoadProject(store()),
            ConfigOpError::LoadLocal(store()),
            ConfigOpError::SaveUser(store()),
            ConfigOpError::SaveProject(store()),
            ConfigOpError::SaveLocal(store()),
            ConfigOpError::ExcludeLocal(fails_to_exclude(Path::new("local")).unwrap_err()),
        ]
    }

    #[test]
    fn every_cause_has_its_own_code() {
        let errors = every_variant();
        let codes: HashSet<&str> = errors.iter().map(ConfigOpError::code).collect();
        assert_eq!(codes.len(), errors.len());
    }

    #[test]
    fn every_cause_has_its_own_text_that_ends_as_a_sentence() {
        let errors = every_variant();
        let texts: HashSet<String> = errors.iter().map(ToString::to_string).collect();
        assert_eq!(texts.len(), errors.len());
        for text in texts {
            assert!(text.ends_with('.'), "{text}");
        }
    }

    #[test]
    fn a_project_that_is_missing_names_the_directory_and_the_scope() {
        let error = ConfigOpError::ProjectRootRequired {
            scope: ConfigScope::Project,
            searched_from: PathBuf::from("somewhere"),
        };
        assert_eq!(
            error.to_string(),
            "No mmcp project at somewhere. Scope project needs a .mmcp.toml."
        );
    }

    #[test]
    fn the_wrapped_failure_stays_a_walkable_source() {
        for error in every_variant() {
            let has_source = !matches!(error, ConfigOpError::ProjectRootRequired { .. });
            assert_eq!(error.source().is_some(), has_source, "{error:?}");
        }
    }
}
