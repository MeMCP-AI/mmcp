//! [`ConfigOpError`], why a `config` operation failed.

use mmcp_core::config::ConfigScope;
use mmcp_git::GitError;
use mmcp_store::StoreError;

/// Failure of a get, set or unset of a configuration key.
/// One variant per cause; the adapters (MCP tool, CLI) map each variant to their own wire form.
/// The cause stays a walkable source.
#[derive(Debug)]
pub enum ConfigOpError {
    /// The scope stores its key in a project file, and no project root resolved.
    ProjectRootRequired {
        /// The scope that needs a project.
        scope: ConfigScope,
    },

    /// The user-level `~/.mmcp/config.toml` could not be loaded.
    LoadUser(StoreError),

    /// The project's `.mmcp.toml` could not be loaded.
    LoadProject(StoreError),

    /// The project's `.mmcp.local.toml` could not be located or loaded.
    LoadLocal(StoreError),

    /// The user-level `~/.mmcp/config.toml` could not be written.
    SaveUser(StoreError),

    /// The project's `.mmcp.toml` could not be written.
    SaveProject(StoreError),

    /// The project's `.mmcp.local.toml` could not be written.
    SaveLocal(StoreError),

    /// The local file could not be kept out of git, so it was not written.
    ExcludeLocal(GitError),
}

impl ConfigOpError {
    /// The underlying store or git failure, `None` for [`ConfigOpError::ProjectRootRequired`].
    #[must_use]
    pub fn source_error(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::ProjectRootRequired { .. } => None,
            Self::LoadUser(source)
            | Self::LoadProject(source)
            | Self::LoadLocal(source)
            | Self::SaveUser(source)
            | Self::SaveProject(source)
            | Self::SaveLocal(source) => Some(source),
            Self::ExcludeLocal(source) => Some(source),
        }
    }
}
