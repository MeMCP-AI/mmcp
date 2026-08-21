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
use tauri::{AppHandle, State};

use crate::error::{GuiError, GuiResult};
use crate::state::{AppState, resolve_reference_point};

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

/// Load the `ProjectConfig` currently active at this session's
/// reference point, if one resolves cleanly.
///
/// `None` covers every case where there is nothing concrete to merge
/// a user-config save against: no reference point and no usable cwd,
/// no `.mmcp.toml` found under it, or a malformed project file.
///
/// The merge check this feeds is best-effort against the ACTIVE
/// project; the single-file `SyncConfig::validate` inside
/// `save_user_config` stays the authoritative floor regardless of
/// whether a project resolves here.
fn active_project_config(app: &AppHandle) -> Option<ProjectConfig> {
    let start = resolve_reference_point(app).or_else(|| std::env::current_dir().ok())?;
    let root = project_config::find_project_root(&start)?;
    project_config::load(&root).ok()
}

/// Reject `user`'s sync config if merging it against `project`'s
/// produces an effective remote set `resolve_effective_remotes` could
/// not resolve at read time (ambiguous default, cross-level name
/// collision, an invalid remote name, a group-less user-level
/// `direct-git` entry, ...).
///
/// Factored out of the command bodies so it is unit-testable without
/// a live `AppHandle`/`State`.
fn reject_unresolvable_merge(user: &UserConfig, project: &ProjectConfig) -> GuiResult<()> {
    mmcp_store::resolve_effective_remotes(user, project)
        .map(|_| ())
        .map_err(GuiError::from)
}

#[tauri::command]
pub async fn save_user_config(
    app: AppHandle,
    state: State<'_, AppState>,
    config: UserConfig,
) -> GuiResult<ResolvedAuthorDto> {
    let home = MmcpHome::discover().map_err(GuiError::from)?;
    // Reject BEFORE writing: a merged set this candidate would
    // produce with the active project (ambiguous default,
    // cross-level collision, ...) fails here at write time, instead
    // of silently landing on disk and only surfacing as a fatal
    // `resolve_effective_remotes` error the next time the app starts.
    if let Some(project_cfg) = active_project_config(&app) {
        reject_unresolvable_merge(&config, &project_cfg)?;
    }
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
    // Same reject-before-write guard as `save_user_config`, merged
    // against the user config instead: this save path always knows
    // its exact project, so the check runs unconditionally rather
    // than best-effort.
    let home = MmcpHome::discover().map_err(GuiError::from)?;
    let user_cfg = home.load_user_config().map_err(GuiError::from)?;
    reject_unresolvable_merge(&user_cfg, &args.config)?;
    project_config::save(root, &args.config).map_err(GuiError::from)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use mmcp_core::config::{Remote, SyncConfig};

    use super::*;

    fn project_cfg(remotes: Vec<Remote>) -> ProjectConfig {
        ProjectConfig {
            project_uuid: mmcp_core::id::ProjectUuid::new(),
            project_slug: None,
            sync: SyncConfig {
                server_url: None,
                remotes,
            },
            project_remote_only: false,
            subscriptions: mmcp_core::config::SubscriptionsConfig::default(),
        }
    }

    fn user_cfg(remotes: Vec<Remote>) -> UserConfig {
        UserConfig {
            sync: SyncConfig {
                server_url: None,
                remotes,
            },
            author: None,
            defaults: None,
            limits: None,
        }
    }

    fn mmcp_server(name: &str, default: bool) -> Remote {
        Remote::MmcpServer {
            name: name.to_string(),
            url: format!("https://{name}.example.com"),
            default,
            include_in_push_all: true,
        }
    }

    /// Two remotes across the merged
    /// user+project set, neither marked default, must be rejected
    /// BEFORE persist, the exact shape `resolve_effective_remotes`
    /// only used to catch on the NEXT load, after a bad config had
    /// already bricked `AppState::discover`.
    #[test]
    fn ambiguous_default_across_merged_levels_is_rejected() {
        let user = user_cfg(vec![mmcp_server("u1", false)]);
        let project = project_cfg(vec![mmcp_server("p1", false)]);

        let err = reject_unresolvable_merge(&user, &project)
            .expect_err("ambiguous merged default must be rejected");

        assert_eq!(
            err.to_string(),
            "store: ambiguous default sync remote: mark exactly one of [u1, p1] as default = true"
        );
    }

    /// Positive control: a merged set that resolves to exactly one
    /// default is accepted, proving the guard does not over-reject.
    #[test]
    fn a_resolvable_merged_default_is_accepted() {
        let user = user_cfg(vec![mmcp_server("u1", false)]);
        let project = project_cfg(vec![mmcp_server("p1", true)]);

        reject_unresolvable_merge(&user, &project).expect("resolvable merge must be accepted");
    }
}
