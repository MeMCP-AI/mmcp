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

/// Map a `ProjectConfig` TOML round-trip failure onto the caller's own `path`.
/// The resulting `StoreError` names the file that failed,
/// instead of losing it behind `ConfigError`'s path-less `#[from]` conversion.
/// `ConfigError` cannot carry a path itself: `ProjectConfig::from_toml`/`to_toml` are generic over any caller.
/// See `home.rs` and `sessions.rs` for the same pattern.
fn attach_path(path: PathBuf, error: ConfigError) -> StoreError {
    match error {
        ConfigError::Parse(source) => StoreError::TomlParse { path, source },
        ConfigError::Render(source) => StoreError::TomlSerialize { path, source },
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
pub fn save(root: &Path, config: &ProjectConfig) -> Result<(), StoreError> {
    let path = config_path_for(root);
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
}
