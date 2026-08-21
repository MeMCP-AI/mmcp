//! Errors returned by the configuration loader and renderer.

use thiserror::Error;

/// Failure modes when reading or writing `.mmcp/config.toml`.
#[derive(Debug, Error)]
pub enum ConfigError {
    /// The TOML text could not be parsed.
    #[error("invalid TOML in mmcp config: {0}")]
    Parse(#[from] toml::de::Error),

    /// The parsed structure could not be serialized back to TOML.
    #[error("failed to render mmcp config: {0}")]
    Render(#[from] toml::ser::Error),

    /// Two entries in the same `sync.remotes` list share the same
    /// `name`. Single-file, single-level check: a name collision
    /// between a project remote and a user remote is rejected by
    /// `mmcp_store::sync::remotes::check_name_collisions`, not this
    /// check.
    #[error("duplicate sync remote name in this config: {name}")]
    DuplicateRemoteName {
        /// The name shared by two or more `remotes` entries.
        name: String,
    },

    /// More than one entry in the same `sync.remotes` list sets
    /// `default = true`. Single-file, single-level check: picking a
    /// winner across project and user levels is
    /// `mmcp_store::sync::remotes::resolve_default_index`'s job, not
    /// this check.
    #[error("more than one default sync remote in this config: {}", names.join(", "))]
    MultipleDefaultRemotes {
        /// Names of every remote in this list marked `default = true`.
        names: Vec<String>,
    },
}
