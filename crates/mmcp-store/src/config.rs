//! Project configuration loader shared by every mmcp consumer.
//!
//! `.mmcp.toml` at a project root describes the project's stable UUID, optional slug,
//! the sync server it talks to, and the group loading preferences.
//! This module owns the walk-up discovery (`find_project_root`), the TOML read (`load`),
//! and the TOML write (`save`).
//! The typed `ProjectConfig` struct itself lives in `mmcp-core::config::project`;
//! this module is the I/O layer.

use std::path::{Path, PathBuf};

use mmcp_core::config::{ConfigError, ProjectConfig};

use crate::error::{FileOperation, StoreError};

/// Map a `ProjectConfig`/`UserConfig` TOML round-trip failure onto
/// the caller's own `path`. The resulting `StoreError` names the
/// file that failed, instead of losing it behind `ConfigError`'s
/// path-less `#[from]` conversion. `ConfigError` cannot carry a path
/// itself: `ProjectConfig::from_toml`/`to_toml` and
/// `UserConfig::from_toml`/`to_toml` are generic over any caller.
/// Exhaustive match, no catch-all: each `ConfigError` variant maps
/// onto its own explicit `StoreError` variant so a caller can tell a
/// TOML syntax error from a semantic-validation failure. Shared by
/// this module (project-level) and `home.rs` (user-level) so the two
/// levels never drift onto different `StoreError` shapes for the
/// same underlying `ConfigError`.
pub(crate) fn attach_path(path: PathBuf, error: ConfigError) -> StoreError {
    match error {
        ConfigError::Parse(source) => StoreError::TomlParse { path, source },
        ConfigError::Render(source) => StoreError::TomlSerialize { path, source },
        ConfigError::DuplicateRemoteName { name } => {
            StoreError::ConfigDuplicateRemoteName { path, name }
        }
        ConfigError::MultipleDefaultRemotes { names } => {
            StoreError::ConfigMultipleDefaultRemotes { path, names }
        }
    }
}

/// Project-level manifest file name.
/// Same as the group repo manifest, a single `.mmcp.toml` convention everywhere.
pub use mmcp_core::manifest::MANIFEST_FILENAME as PROJECT_MANIFEST;

/// Locate the project root by walking up from `start` until a `.mmcp.toml` file is found.
/// Returns `None` when no ancestor contains the manifest.
pub fn find_project_root(start: &Path) -> Option<PathBuf> {
    let mut cursor = Some(start.to_path_buf());
    while let Some(dir) = cursor {
        if dir.join(PROJECT_MANIFEST).exists() {
            return Some(dir);
        }
        cursor = dir.parent().map(Path::to_path_buf);
    }
    None
}

/// Full path to the manifest file inside a project root.
#[must_use]
pub fn config_path_for(root: &Path) -> PathBuf {
    root.join(PROJECT_MANIFEST)
}

/// Load the `ProjectConfig` from `root/.mmcp.toml`.
pub fn load(root: &Path) -> Result<ProjectConfig, StoreError> {
    let path = config_path_for(root);
    let text = std::fs::read_to_string(&path)
        .map_err(|source| StoreError::io(path.clone(), FileOperation::Read, source))?;
    ProjectConfig::from_toml(&text).map_err(|error| attach_path(path, error))
}

/// Render `config` into the project's `.mmcp.toml`.
/// Overwrites any existing file.
///
/// Validates `config.sync` (same [`mmcp_core::config::SyncConfig::validate`]
/// check `ProjectConfig::from_toml` runs on read) BEFORE writing, so
/// this function can never persist a `.mmcp.toml` that a subsequent
/// `load` would then reject: a duplicate remote name or more than
/// one `default = true` remote fails here, at write time, instead of
/// silently landing on disk and only surfacing on the next load.
pub fn save(root: &Path, config: &ProjectConfig) -> Result<(), StoreError> {
    let path = config_path_for(root);
    config
        .sync
        .validate()
        .map_err(|error| attach_path(path.clone(), error))?;
    let text = config
        .to_toml()
        .map_err(|error| attach_path(path.clone(), error))?;
    std::fs::write(&path, text)
        .map_err(|source| StoreError::io(path, FileOperation::Write, source))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    /// A malformed `.mmcp.toml` surfaces as `StoreError::TomlParse` naming the failing path.
    #[test]
    fn load_malformed_toml_returns_typed_parse_error_with_path() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let root = tmp.path();
        let config_path = config_path_for(root);
        std::fs::write(&config_path, "not = [valid").expect("write malformed toml");

        let err = load(root).expect_err("malformed TOML must not parse");

        match &err {
            StoreError::TomlParse { path, .. } => assert_eq!(path, &config_path),
            other => panic!("expected StoreError::TomlParse, got {other:?}"),
        }
        assert!(
            std::error::Error::source(&err)
                .and_then(|s| s.downcast_ref::<toml::de::Error>())
                .is_some(),
            "source must be the real toml::de::Error, not a stringified copy"
        );
    }

    /// A `.mmcp.toml` declaring two `[[sync.remotes]]` entries with
    /// the same `name` surfaces as `StoreError::ConfigDuplicateRemoteName`
    /// naming the failing path, exercising `attach_path`'s
    /// `ConfigError::DuplicateRemoteName` arm end to end through `load`.
    #[test]
    fn load_duplicate_remote_names_returns_typed_error_with_path() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let root = tmp.path();
        let config_path = config_path_for(root);
        std::fs::write(
            &config_path,
            r#"
project_uuid = "018f7c3e-4d2a-7b1f-9e5c-6a8d2f0b4c91"

[[sync.remotes]]
kind = "mmcp-server"
name = "primary"
url = "https://a.example.com"

[[sync.remotes]]
kind = "direct-git"
name = "primary"
url = "ssh://git@example.com/b.git"
"#,
        )
        .expect("write config with duplicate remote names");

        let err = load(root).expect_err("duplicate remote name must not load");

        match &err {
            StoreError::ConfigDuplicateRemoteName { path, name } => {
                assert_eq!(path, &config_path);
                assert_eq!(name, "primary");
            }
            other => panic!("expected StoreError::ConfigDuplicateRemoteName, got {other:?}"),
        }
    }

    /// A `.mmcp.toml` declaring two `default = true` remotes surfaces
    /// as `StoreError::ConfigMultipleDefaultRemotes` naming the
    /// failing path, exercising `attach_path`'s
    /// `ConfigError::MultipleDefaultRemotes` arm end to end through
    /// `load`.
    #[test]
    fn load_multiple_default_remotes_returns_typed_error_with_path() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let root = tmp.path();
        let config_path = config_path_for(root);
        std::fs::write(
            &config_path,
            r#"
project_uuid = "018f7c3e-4d2a-7b1f-9e5c-6a8d2f0b4c91"

[[sync.remotes]]
kind = "mmcp-server"
name = "primary"
url = "https://a.example.com"
default = true

[[sync.remotes]]
kind = "mmcp-server"
name = "secondary"
url = "https://b.example.com"
default = true
"#,
        )
        .expect("write config with two default remotes");

        let err = load(root).expect_err("two default remotes must not load");

        match &err {
            StoreError::ConfigMultipleDefaultRemotes { path, names } => {
                assert_eq!(path, &config_path);
                assert_eq!(names, &vec!["primary".to_string(), "secondary".to_string()]);
            }
            other => panic!("expected StoreError::ConfigMultipleDefaultRemotes, got {other:?}"),
        }
    }

    fn project_with_remotes(remotes: Vec<mmcp_core::config::Remote>) -> ProjectConfig {
        ProjectConfig {
            project_uuid: mmcp_core::id::ProjectUuid::from_uuid(
                uuid::Uuid::parse_str("018f7c3e-4d2a-7b1f-9e5c-6a8d2f0b4c91").unwrap(),
            ),
            project_slug: None,
            sync: mmcp_core::config::SyncConfig {
                server_url: None,
                remotes,
            },
            project_remote_only: false,
            subscriptions: mmcp_core::config::SubscriptionsConfig::default(),
        }
    }

    /// A `ProjectConfig` built programmatically (never round-tripped
    /// through TOML text) with two `[[sync.remotes]]` entries sharing
    /// a `name` must be REJECTED by `save` itself, not only by
    /// `from_toml` on a later load: this is the write path, so
    /// `save_project_config` never persists a config that then fails
    /// to load back.
    #[test]
    fn save_rejects_a_programmatically_built_config_with_duplicate_remote_names() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let root = tmp.path();
        let cfg = project_with_remotes(vec![
            mmcp_core::config::Remote::MmcpServer {
                name: "primary".to_string(),
                url: "https://a.example.com".to_string(),
                default: false,
                include_in_push_all: true,
            },
            mmcp_core::config::Remote::MmcpServer {
                name: "primary".to_string(),
                url: "https://b.example.com".to_string(),
                default: false,
                include_in_push_all: true,
            },
        ]);

        let err = save(root, &cfg).expect_err("duplicate remote name must not save");

        match &err {
            StoreError::ConfigDuplicateRemoteName { name, .. } => assert_eq!(name, "primary"),
            other => panic!("expected StoreError::ConfigDuplicateRemoteName, got {other:?}"),
        }
        assert!(
            !config_path_for(root).exists(),
            "a rejected save must not leave a partial .mmcp.toml on disk"
        );
    }

    /// Same write-path guard, for the other `SyncConfig::validate`
    /// failure mode: two remotes both marked `default = true`.
    #[test]
    fn save_rejects_a_programmatically_built_config_with_two_default_remotes() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let root = tmp.path();
        let cfg = project_with_remotes(vec![
            mmcp_core::config::Remote::MmcpServer {
                name: "primary".to_string(),
                url: "https://a.example.com".to_string(),
                default: true,
                include_in_push_all: true,
            },
            mmcp_core::config::Remote::MmcpServer {
                name: "secondary".to_string(),
                url: "https://b.example.com".to_string(),
                default: true,
                include_in_push_all: true,
            },
        ]);

        let err = save(root, &cfg).expect_err("two default remotes must not save");

        match &err {
            StoreError::ConfigMultipleDefaultRemotes { names, .. } => {
                assert_eq!(names, &vec!["primary".to_string(), "secondary".to_string()]);
            }
            other => panic!("expected StoreError::ConfigMultipleDefaultRemotes, got {other:?}"),
        }
        assert!(
            !config_path_for(root).exists(),
            "a rejected save must not leave a partial .mmcp.toml on disk"
        );
    }
}
