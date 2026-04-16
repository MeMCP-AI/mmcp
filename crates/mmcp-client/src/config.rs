//! Project configuration loader for mmcp-client.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use mmcp_core::config::ProjectConfig;

/// Project-level manifest file name. Same as the group repo
/// manifest - a single `.mmcp.toml` convention everywhere.
pub use mmcp_core::manifest::MANIFEST_FILENAME as PROJECT_MANIFEST;

/// Locate the project root by walking up from `start` until a
/// `.mmcp.toml` file is found. Returns `None` when no ancestor
/// contains the manifest.
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
pub fn load(root: &Path) -> Result<ProjectConfig> {
    let path = config_path_for(root);
    let text = std::fs::read_to_string(&path)
        .with_context(|| format!("reading {}", path.display()))?;
    ProjectConfig::from_toml(&text)
        .with_context(|| format!("parsing {}", path.display()))
}

/// Render `config` into the project's `.mmcp.toml`. Overwrites
/// any existing file.
pub fn save(root: &Path, config: &ProjectConfig) -> Result<()> {
    let path = config_path_for(root);
    let text = config
        .to_toml()
        .with_context(|| format!("rendering {}", path.display()))?;
    std::fs::write(&path, text).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}
