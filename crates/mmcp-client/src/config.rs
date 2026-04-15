//! Project configuration loader for mmcp-client.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use mmcp_core::config::ProjectConfig;

/// Project-level configuration directory (committed to the
/// project's own version control).
pub const PROJECT_CONFIG_DIR: &str = ".mmcp";

/// Per-project configuration file name inside [`PROJECT_CONFIG_DIR`].
pub const PROJECT_CONFIG_FILE: &str = "config.toml";

/// Locate the `.mmcp` directory by walking up from the current
/// working directory until one is found. Returns `None` when no
/// ancestor contains a `.mmcp/config.toml`.
pub fn find_project_root(start: &Path) -> Option<PathBuf> {
    let mut cursor = Some(start.to_path_buf());
    while let Some(dir) = cursor {
        let candidate = dir.join(PROJECT_CONFIG_DIR).join(PROJECT_CONFIG_FILE);
        if candidate.exists() {
            return Some(dir);
        }
        cursor = dir.parent().map(Path::to_path_buf);
    }
    None
}

/// Full path to the config file inside a project root.
#[must_use]
pub fn config_path_for(root: &Path) -> PathBuf {
    root.join(PROJECT_CONFIG_DIR).join(PROJECT_CONFIG_FILE)
}

/// Load the `ProjectConfig` anchored at `root`.
pub fn load(root: &Path) -> Result<ProjectConfig> {
    let path = config_path_for(root);
    let text = std::fs::read_to_string(&path)
        .with_context(|| format!("reading {}", path.display()))?;
    ProjectConfig::from_toml(&text)
        .with_context(|| format!("parsing {}", path.display()))
}

/// Render `config` into the project's config file, creating the
/// `.mmcp` directory if it does not yet exist. Overwrites any
/// existing file.
pub fn save(root: &Path, config: &ProjectConfig) -> Result<()> {
    let dir = root.join(PROJECT_CONFIG_DIR);
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("creating {}", dir.display()))?;
    let path = config_path_for(root);
    let text = config
        .to_toml()
        .with_context(|| format!("rendering {}", path.display()))?;
    std::fs::write(&path, text).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}
