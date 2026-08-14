//! Project configuration loader shared by every mmcp consumer.
//!
//! `.mmcp.toml` at a project root describes the project's stable UUID, optional slug,
//! the sync server it talks to, and the group loading preferences.
//! This module owns the walk-up discovery (`find_project_root`), the TOML read (`load`),
//! and the TOML write (`save`).
//! The typed `ProjectConfig` struct itself lives in `mmcp-core::config::project`;
//! this module is the I/O layer.

use std::path::{Path, PathBuf};

use mmcp_core::config::ProjectConfig;

use crate::error::{FileOperation, StoreError};

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
    let text = std::fs::read_to_string(&path).map_err(|source| StoreError::Io {
        path,
        operation: FileOperation::Read,
        source,
    })?;
    Ok(ProjectConfig::from_toml(&text)?)
}

/// Render `config` into the project's `.mmcp.toml`.
/// Overwrites any existing file.
pub fn save(root: &Path, config: &ProjectConfig) -> Result<(), StoreError> {
    let path = config_path_for(root);
    let text = config.to_toml()?;
    std::fs::write(&path, text).map_err(|source| StoreError::Io {
        path,
        operation: FileOperation::Write,
        source,
    })?;
    Ok(())
}
