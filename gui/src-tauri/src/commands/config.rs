//! User + Project config CRUD.
//!
//! The frontend Settings window drives these to let the user edit
//! `~/.mmcp/config.toml` (author identity, default sync server, …)
//! and the project's own `.mmcp.toml` (project slug, sync server,
//! group/language lists) without ever hand-editing a TOML file.

use std::path::{Path, PathBuf};

use mmcp_core::config::{ProjectConfig, UserConfig};
use mmcp_store::{MmcpHome, ResolvedAuthor, config as project_config};
use serde::{Deserialize, Serialize};
use tauri::State;

use crate::error::{GuiError, GuiResult};
use crate::state::AppState;

#[derive(Debug, Clone, Serialize)]
pub struct ResolvedAuthorDto {
    pub name: String,
    pub email: String,
}

impl From<ResolvedAuthor> for ResolvedAuthorDto {
    fn from(a: ResolvedAuthor) -> Self {
        Self {
            name: a.name,
            email: a.email,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct LoadedUserConfigDto {
    /// Absolute path to the `~/.mmcp/config.toml` file. Returned so
    /// the UI can tell the user exactly where the edits are landing.
    pub path: String,
    pub config: UserConfig,
    pub resolved_author: ResolvedAuthorDto,
}

#[derive(Debug, Serialize)]
pub struct LoadedProjectConfigDto {
    /// Directory containing the resolved `.mmcp.toml`, or `None`
    /// when no ancestor of the reference-point carries one yet.
    pub root: Option<String>,
    /// Parsed config — `None` when `root` is `None`.
    pub config: Option<ProjectConfig>,
}

#[derive(Debug, Deserialize)]
pub struct SaveProjectConfigArgs {
    /// Directory to write `.mmcp.toml` into. Must already exist.
    pub root: String,
    pub config: ProjectConfig,
}

#[tauri::command]
pub async fn load_user_config(state: State<'_, AppState>) -> GuiResult<LoadedUserConfigDto> {
    let home = MmcpHome::discover().map_err(GuiError::from)?;
    let cfg = home.load_user_config().map_err(GuiError::from)?;
    let author = state.author.read().await.clone();
    Ok(LoadedUserConfigDto {
        path: home.user_config_path().to_string_lossy().into_owned(),
        config: cfg,
        resolved_author: author.into(),
    })
}

#[tauri::command]
pub async fn save_user_config(
    state: State<'_, AppState>,
    config: UserConfig,
) -> GuiResult<ResolvedAuthorDto> {
    let home = MmcpHome::discover().map_err(GuiError::from)?;
    home.save_user_config(&config).map_err(GuiError::from)?;
    let fresh = state.reload_author().await?;
    Ok(fresh.into())
}

fn normalise_dir(path: Option<String>) -> Option<PathBuf> {
    let raw = path?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let candidate = PathBuf::from(trimmed);
    candidate.is_dir().then_some(candidate)
}

#[tauri::command]
pub async fn load_project_config(path: Option<String>) -> GuiResult<LoadedProjectConfigDto> {
    let start: PathBuf = match normalise_dir(path) {
        Some(p) => p,
        None => match std::env::current_dir() {
            Ok(cwd) => cwd,
            Err(e) => return Err(GuiError::CurrentDirUnavailable(e)),
        },
    };
    let Some(root) = project_config::find_project_root(&start) else {
        return Ok(LoadedProjectConfigDto {
            root: None,
            config: None,
        });
    };
    let cfg = project_config::load(&root).map_err(GuiError::from)?;
    Ok(LoadedProjectConfigDto {
        root: Some(root.to_string_lossy().into_owned()),
        config: Some(cfg),
    })
}

#[tauri::command]
pub async fn save_project_config(args: SaveProjectConfigArgs) -> GuiResult<()> {
    let root = Path::new(&args.root);
    if !root.is_dir() {
        return Err(GuiError::NotADirectory {
            path: root.to_path_buf(),
        });
    }
    project_config::save(root, &args.config).map_err(GuiError::from)?;
    Ok(())
}
