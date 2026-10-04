//! [`ConfigEnvironment`], what a `config` operation reads and writes through.

use std::path::Path;

use mmcp_core::config::{LOCAL_CONFIG_EXCLUDE_PATTERN, NoticeLaunch};
use mmcp_git::GitError;
use mmcp_git::checkout::{Exclusion, exclude_path_globally};
use mmcp_store::home::MmcpHome;

/// Keeps a local configuration file out of git, reporting what it did.
pub type LocalExclusion = fn(&Path) -> Result<Exclusion, GitError>;

/// The served home, the values the process was launched with, and the git exclusion of the local file.
pub struct ConfigEnvironment<'a> {
    /// The home whose `config.toml` is the user scope.
    pub home: &'a MmcpHome,
    /// The launch flag and variable values of both keys.
    pub launch: &'a NoticeLaunch,
    /// How the local file is kept out of git before its first write.
    pub exclude_local: LocalExclusion,
}

impl<'a> ConfigEnvironment<'a> {
    /// The production environment: the local file is excluded through the user's global git excludes file.
    #[must_use]
    pub fn new(home: &'a MmcpHome, launch: &'a NoticeLaunch) -> Self {
        Self {
            home,
            launch,
            exclude_local: exclude_through_global_excludes,
        }
    }
}

/// Exclude `path` through the user's global git excludes file with the local-file pattern.
fn exclude_through_global_excludes(path: &Path) -> Result<Exclusion, GitError> {
    exclude_path_globally(path, LOCAL_CONFIG_EXCLUDE_PATTERN)
}
