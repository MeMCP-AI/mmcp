//! `mmcp init` implementation.

use anyhow::{Context, Result, bail};
use mmcp_core::config::{GroupsConfig, LanguagesConfig, ProjectConfig};
use mmcp_core::id::ProjectUuid;

use crate::config::{config_path_for, save};

/// Initialize a new mmcp project rooted at the current working
/// directory.
pub async fn run() -> Result<()> {
    let cwd = std::env::current_dir().context("reading current working directory")?;
    let existing = config_path_for(&cwd);
    if existing.exists() {
        bail!(
            "mmcp project already initialized at {}",
            existing.display()
        );
    }

    let config = ProjectConfig {
        project_uuid: ProjectUuid::new(),
        sync: None,
        groups: GroupsConfig::default(),
        languages: LanguagesConfig::default(),
    };
    save(&cwd, &config)?;

    println!(
        "initialized mmcp project {} at {}",
        config.project_uuid,
        cwd.display()
    );
    Ok(())
}
