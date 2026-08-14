//! `mmcp init project` implementation.
//!
//! The project is bootstrapped by a single command: `.mmcp.toml`
//! and the backing bare git repo are both created (or, more often,
//! one of them is created next to the other that is already there).
//! `--config-only` exists for operators who want to write just the
//! project config up front: useful when adopting a server-side
//! project that a follow-up `mmcp pull` will populate.
//!
//! Idempotency rule: *never rewrite* either artifact. The bare repo
//! is untouched once it exists, and `.mmcp.toml` is only ever
//! enriched (specifically, an absent `project_slug` gets backfilled).
//! Every other difference between args and on-disk state (a
//! `--project-uuid` that disagrees with the stored one, a `--slug`
//! that disagrees with the stored one) is an error so the operator
//! never silently ends up with the wrong project identity.

use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use inquire::Text;
use mmcp_core::config::ProjectConfig;
use mmcp_core::id::{GroupId, ProjectUuid, UserId};
use mmcp_core::manifest::GroupManifest;
use mmcp_git::{GitBackend, NativeBackend};
use thiserror::Error;
use uuid::Uuid;

use mmcp_store::config::{find_project_root, load, save};
use mmcp_store::groups::GroupIndex;
use mmcp_store::home::MmcpHome;
use mmcp_store::memory::validate_slug_segment;

// ── CLI args ─────────────────────────────────────────────────────────

/// Flags accepted by `mmcp init project`.
#[derive(Debug, Clone, clap::Args)]
pub struct ProjectArgs {
    /// Group slug (kebab-case). Absent on a TTY prompts for one
    /// using the slugified project directory basename as the
    /// default; absent on a non-TTY with no stored slug errors so
    /// scripted runs have to pick a name explicitly.
    #[arg(long)]
    pub slug: Option<String>,

    /// Write `.mmcp.toml` only. Skip bare-repo creation. Useful when
    /// adopting a project whose repo is hosted on a server and will
    /// land locally via the first `mmcp pull`.
    #[arg(long)]
    pub config_only: bool,

    /// Adopt an explicit project UUID instead of minting a fresh v7.
    /// Rejected if `.mmcp.toml` already exists with a different UUID.
    #[arg(long)]
    pub project_uuid: Option<Uuid>,
}

// ── Shared types ─────────────────────────────────────────────────────

/// Options consumed by the shared bootstrap helper. Kept as a
/// struct rather than a long arg list so CLI and MCP callers
/// converge on one shape; optional fields default via `Default`.
#[derive(Debug, Clone, Default)]
pub struct InitProjectOptions {
    pub slug: Option<String>,
    pub config_only: bool,
    pub project_uuid: Option<Uuid>,
}

/// Successful outcome of a project-bootstrap call.
#[derive(Debug, Clone)]
pub struct ProjectGroupReport {
    pub project_uuid: ProjectUuid,
    pub project_root: PathBuf,
    pub slug: String,
    pub group_id: GroupId,
    /// `None` when `config_only` was set and the repo is still
    /// absent; `Some(path)` whenever a repo exists on disk (either
    /// created by this call or already present).
    pub repo_path: Option<PathBuf>,
    /// `true` when this call wrote `.mmcp.toml` for the first time,
    /// `false` when the config was already there. Slug backfill on
    /// an existing config does not flip this to `true`: the config
    /// is enriched, not replaced.
    pub created_config: bool,
    /// `true` when this call created the bare repo, `false` for a
    /// pre-existing repo or a `config_only` run.
    pub created_repo: bool,
}

/// Structured failures emitted by the bootstrap helper.
///
/// The MCP tool maps each variant to a distinct `code` so AI
/// clients branch on state rather than parsing human strings.
#[derive(Debug, Error)]
pub enum InitProjectError {
    #[error(
        "invalid slug `{slug}`: must be 1-128 lowercase alphanumeric chars or hyphens, no leading/trailing/consecutive hyphens"
    )]
    InvalidSlug { slug: String },

    #[error("failed to load project config: {0}")]
    ConfigLoadFailed(String),

    #[error("failed to write project config: {0}")]
    ConfigWriteFailed(String),

    #[error(
        "supplied project_uuid {got} disagrees with the one already stored in .mmcp.toml ({expected}); refusing to rewrite project identity"
    )]
    ProjectUuidMismatch { expected: Uuid, got: Uuid },

    #[error(
        "supplied slug `{got}` disagrees with the one already stored in .mmcp.toml (`{expected}`); refusing to rewrite project slug"
    )]
    SlugMismatch { expected: String, got: String },

    #[error("slug required: pass `--slug <slug>` (or supply it in the `slug` tool argument)")]
    SlugRequired,

    #[error("git backend error: {0}")]
    GitBackend(String),

    #[error("group index refresh failed: {0}")]
    IndexRefreshFailed(String),
}

// ── CLI entry ────────────────────────────────────────────────────────

/// CLI entry for `mmcp init project`.
pub async fn run_project(args: ProjectArgs) -> Result<()> {
    let cwd = std::env::current_dir().context("reading current working directory")?;
    let home = MmcpHome::discover()?;

    let opts = InitProjectOptions {
        slug: args.slug,
        config_only: args.config_only,
        project_uuid: args.project_uuid,
    };

    let (backend, groups) = home.init_backend().await?;
    let report = bootstrap_project(
        &backend, &groups, &cwd, &opts, /* tty_slug_prompt */ true,
    )
    .await
    .map_err(anyhow::Error::from)?;

    print_report(&report);
    Ok(())
}

/// MCP entry: reuses state the server already holds. `tty_slug_prompt`
/// is always `false`; MCP callers either supply a slug up front, rely
/// on a stored `project_slug`, or receive `SlugRequired`.
pub async fn create_project_group_from_state(
    backend: &Arc<NativeBackend>,
    groups: &GroupIndex,
    cwd: &Path,
    opts: &InitProjectOptions,
) -> Result<ProjectGroupReport, InitProjectError> {
    bootstrap_project(backend, groups, cwd, opts, false).await
}

// ── Core orchestrator ────────────────────────────────────────────────

async fn bootstrap_project(
    backend: &Arc<NativeBackend>,
    groups: &GroupIndex,
    cwd: &Path,
    opts: &InitProjectOptions,
    tty_slug_prompt: bool,
) -> Result<ProjectGroupReport, InitProjectError> {
    // 1. Discover or mint `.mmcp.toml`. `project_root` is always the
    //    directory that holds the config once we return: either the
    //    discovered ancestor or `cwd` when this call created it.
    let (mut cfg, project_root, created_config) = load_or_mint_config(cwd, opts.project_uuid)?;

    // 2. Resolve the slug from args → stored config → optional TTY
    //    prompt. Validate once, end-to-end: whatever we resolve will
    //    be stored in both the config and (absent --config-only) the
    //    group manifest, so any invalid value short-circuits here.
    let slug = resolve_slug(
        opts.slug.as_deref(),
        cfg.project_slug.as_deref(),
        cwd,
        tty_slug_prompt,
    )?;
    validate_slug_segment(&slug)
        .map_err(|_| InitProjectError::InvalidSlug { slug: slug.clone() })?;

    // 3. Backfill `project_slug` if the existing config lacked one.
    //    Any other disagreement already errored out in `resolve_slug`,
    //    so at this point either the stored slug matches or was
    //    absent.
    let mut config_touched = false;
    if cfg.project_slug.as_deref() != Some(slug.as_str()) {
        cfg.project_slug = Some(slug.clone());
        config_touched = true;
    }
    if created_config || config_touched {
        save(&project_root, &cfg)
            .map_err(|e| InitProjectError::ConfigWriteFailed(e.to_string()))?;
    }

    // 4. Decide what to do with the bare repo.
    let project_uuid = cfg.project_uuid;
    let group_id = GroupId::from_uuid(*project_uuid.as_uuid());
    let repo_path = backend.repo_path(*project_uuid.as_uuid());
    let repo_exists = repo_path.exists();

    if opts.config_only {
        // Config-only path: if the repo is already there we still
        // refresh the index so the caller sees consistent state, but
        // we don't claim credit for creating it.
        if repo_exists {
            groups
                .refresh()
                .await
                .map_err(|e| InitProjectError::IndexRefreshFailed(e.to_string()))?;
        }
        return Ok(ProjectGroupReport {
            project_uuid,
            project_root,
            slug,
            group_id,
            repo_path: if repo_exists { Some(repo_path) } else { None },
            created_config,
            created_repo: false,
        });
    }

    if repo_exists {
        groups
            .refresh()
            .await
            .map_err(|e| InitProjectError::IndexRefreshFailed(e.to_string()))?;
        return Ok(ProjectGroupReport {
            project_uuid,
            project_root,
            slug,
            group_id,
            repo_path: Some(repo_path),
            created_config,
            created_repo: false,
        });
    }

    // Fresh repo.
    // Owner is a v7 UUID; ownership semantics remain deferred to the auth track.
    // `create_group_repo` records the owner once and never overwrites it,
    // so regeneration on subsequent calls is harmless (they hit the `repo_exists` short-circuit above).
    let owner = UserId::new();
    let manifest = GroupManifest::new_user_owned(group_id, slug.clone(), owner);
    backend
        .create_group_repo(&manifest)
        .await
        .map_err(|e| InitProjectError::GitBackend(e.to_string()))?;
    groups
        .refresh()
        .await
        .map_err(|e| InitProjectError::IndexRefreshFailed(e.to_string()))?;

    Ok(ProjectGroupReport {
        project_uuid,
        project_root,
        slug,
        group_id,
        repo_path: Some(repo_path),
        created_config,
        created_repo: true,
    })
}

// ── Small typed helpers ──────────────────────────────────────────────

/// Discover the enclosing project config, or mint a fresh one.
///
/// Returns `(config, project_root, created_config)`. When the walk up
/// from `cwd` finds a `.mmcp.toml`, that file is loaded and
/// `created_config = false`. Otherwise a fresh `ProjectConfig` is
/// constructed (UUID from `explicit_uuid` when supplied, fresh v7
/// otherwise), `created_config = true`, and the project root is `cwd`.
///
/// Validation of `explicit_uuid` against an already-stored UUID is
/// performed here so subsequent steps don't have to re-check it.
fn load_or_mint_config(
    cwd: &Path,
    explicit_uuid: Option<Uuid>,
) -> Result<(ProjectConfig, PathBuf, bool), InitProjectError> {
    if let Some(root) = find_project_root(cwd) {
        let cfg = load(&root).map_err(|e| InitProjectError::ConfigLoadFailed(e.to_string()))?;
        if let Some(given) = explicit_uuid
            && *cfg.project_uuid.as_uuid() != given
        {
            return Err(InitProjectError::ProjectUuidMismatch {
                expected: *cfg.project_uuid.as_uuid(),
                got: given,
            });
        }
        return Ok((cfg, root, false));
    }
    let project_uuid = match explicit_uuid {
        Some(given) => ProjectUuid::from_uuid(given),
        None => ProjectUuid::new(),
    };
    let cfg = ProjectConfig {
        project_uuid,
        project_slug: None,
        sync: None,
        subscriptions: Default::default(),
    };
    Ok((cfg, cwd.to_path_buf(), true))
}

/// Resolve the slug from the precedence chain:
///
/// 1. `arg_slug`: whatever the caller passed explicitly.
/// 2. `config_slug`: the stored `project_slug` in `.mmcp.toml`.
/// 3. TTY prompt (only when `tty_slug_prompt` is true and stdin is a
///    terminal), defaulting to the slugified project dir basename.
/// 4. [`InitProjectError::SlugRequired`].
///
/// Errors [`SlugMismatch`] when the caller's slug disagrees with an
/// already-stored slug. A matching arg is accepted, useful for
/// automation that passes the slug defensively even when it's
/// already recorded.
fn resolve_slug(
    arg_slug: Option<&str>,
    config_slug: Option<&str>,
    cwd: &Path,
    tty_slug_prompt: bool,
) -> Result<String, InitProjectError> {
    match (arg_slug, config_slug) {
        (Some(arg), Some(stored)) if arg != stored => Err(InitProjectError::SlugMismatch {
            expected: stored.to_string(),
            got: arg.to_string(),
        }),
        (Some(arg), _) => Ok(arg.to_string()),
        (None, Some(stored)) => Ok(stored.to_string()),
        (None, None) if tty_slug_prompt && std::io::stdin().is_terminal() => {
            let default = default_slug_from_cwd(cwd);
            Text::new("Group slug:")
                .with_default(&default)
                .prompt()
                .map_err(|_| InitProjectError::SlugRequired)
        }
        (None, None) => Err(InitProjectError::SlugRequired),
    }
}

/// Derive a default slug from the project directory basename. Falls
/// back to `"project"` when `cwd` has no nameable terminal component
/// so the interactive prompt always has something to offer.
fn default_slug_from_cwd(cwd: &Path) -> String {
    cwd.file_name()
        .and_then(|os| os.to_str())
        .map(slug::slugify)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "project".to_string())
}

fn print_report(report: &ProjectGroupReport) {
    let config_line = if report.created_config {
        "wrote .mmcp.toml"
    } else {
        ".mmcp.toml already present"
    };
    let repo_line = match (&report.repo_path, report.created_repo) {
        (Some(path), true) => format!("created group repo at {}", path.display()),
        (Some(path), false) => {
            format!("group repo already present at {}", path.display())
        }
        (None, _) => "skipped group repo (config-only)".to_string(),
    };
    println!(
        "project {} ({}): {}, {}",
        report.slug, report.project_uuid, config_line, repo_line
    );
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// Assemble a tempdir-backed `(backend, groups, project_root)`
    /// fixture. The project config is written by the helper under
    /// test, not by the fixture: callers pass a fresh tempdir path.
    async fn test_fixture() -> (Arc<NativeBackend>, GroupIndex, TempDir) {
        let tmp = TempDir::new().expect("tempdir");
        let repos_root = tmp.path().join("repos");
        std::fs::create_dir_all(&repos_root).expect("repos root");
        let backend = Arc::new(NativeBackend::new(&repos_root).expect("backend"));
        let groups = GroupIndex::build(repos_root, backend.clone())
            .await
            .expect("group index");
        (backend, groups, tmp)
    }

    // ── pure helpers ──────────────────────────────────────────────

    #[test]
    fn default_slug_from_cwd_slugifies_dir_basename() {
        let result = default_slug_from_cwd(Path::new("/srv/My Awesome Project"));
        assert_eq!(result, "my-awesome-project");
    }

    #[test]
    fn default_slug_from_cwd_falls_back_for_unnameable_paths() {
        assert_eq!(default_slug_from_cwd(Path::new("/")), "project");
    }

    #[test]
    fn resolve_slug_prefers_arg_over_config() {
        let s = resolve_slug(
            Some("from-arg"),
            Some("from-config"),
            Path::new("/x"),
            false,
        );
        assert!(matches!(s, Err(InitProjectError::SlugMismatch { .. })));
    }

    #[test]
    fn resolve_slug_uses_config_when_arg_is_absent() {
        let s =
            resolve_slug(None, Some("from-config"), Path::new("/x"), false).expect("config slug");
        assert_eq!(s, "from-config");
    }

    #[test]
    fn resolve_slug_matching_arg_and_config_is_accepted() {
        let s = resolve_slug(Some("same"), Some("same"), Path::new("/x"), false).expect("matching");
        assert_eq!(s, "same");
    }

    #[test]
    fn resolve_slug_returns_required_when_no_source_and_no_tty() {
        let s = resolve_slug(None, None, Path::new("/x"), false);
        assert!(matches!(s, Err(InitProjectError::SlugRequired)));
    }

    // ── orchestrator paths ────────────────────────────────────────

    #[tokio::test]
    async fn init_project_writes_both_config_and_repo_from_scratch() {
        let (backend, groups, tmp) = test_fixture().await;
        let project_root = tmp.path().join("project");
        std::fs::create_dir_all(&project_root).expect("project root");
        let opts = InitProjectOptions {
            slug: Some("team-rust".into()),
            ..Default::default()
        };

        let report = bootstrap_project(&backend, &groups, &project_root, &opts, false)
            .await
            .expect("bootstrap");

        assert!(report.created_config);
        assert!(report.created_repo);
        assert_eq!(report.slug, "team-rust");
        assert!(project_root.join(".mmcp.toml").exists());
        assert!(report.repo_path.is_some());
        assert!(report.repo_path.unwrap().exists());
        assert!(
            groups
                .get(&GroupId::from_uuid(*report.project_uuid.as_uuid()))
                .await
                .is_some()
        );
    }

    #[tokio::test]
    async fn init_project_backfills_slug_into_existing_config() {
        let (backend, groups, tmp) = test_fixture().await;
        let project_root = tmp.path().join("project");
        std::fs::create_dir_all(&project_root).expect("project root");

        // Seed a pre-slug config with no `project_slug` field.
        let stored_uuid = ProjectUuid::new();
        let cfg = ProjectConfig {
            project_uuid: stored_uuid,
            project_slug: None,
            sync: None,
            subscriptions: Default::default(),
        };
        save(&project_root, &cfg).expect("seed config");

        let opts = InitProjectOptions {
            slug: Some("team-rust".into()),
            ..Default::default()
        };
        let report = bootstrap_project(&backend, &groups, &project_root, &opts, false)
            .await
            .expect("bootstrap");

        assert!(
            !report.created_config,
            "config was pre-seeded; bootstrap should not claim creation",
        );
        assert!(report.created_repo, "repo was absent and must be created");
        assert_eq!(report.project_uuid, stored_uuid, "UUID must not change");

        let reloaded = load(&project_root).expect("reload");
        assert_eq!(reloaded.project_slug.as_deref(), Some("team-rust"));
    }

    #[tokio::test]
    async fn init_project_rejects_slug_mismatch_against_existing_config() {
        let (backend, groups, tmp) = test_fixture().await;
        let project_root = tmp.path().join("project");
        std::fs::create_dir_all(&project_root).expect("project root");
        let cfg = ProjectConfig {
            project_uuid: ProjectUuid::new(),
            project_slug: Some("team-rust".into()),
            sync: None,
            subscriptions: Default::default(),
        };
        save(&project_root, &cfg).expect("seed config");

        let opts = InitProjectOptions {
            slug: Some("different".into()),
            ..Default::default()
        };
        let err = bootstrap_project(&backend, &groups, &project_root, &opts, false)
            .await
            .expect_err("must reject slug mismatch");
        assert!(matches!(err, InitProjectError::SlugMismatch { .. }));
    }

    #[tokio::test]
    async fn init_project_rejects_project_uuid_mismatch() {
        let (backend, groups, tmp) = test_fixture().await;
        let project_root = tmp.path().join("project");
        std::fs::create_dir_all(&project_root).expect("project root");
        let stored_uuid = ProjectUuid::new();
        let cfg = ProjectConfig {
            project_uuid: stored_uuid,
            project_slug: Some("team-rust".into()),
            sync: None,
            subscriptions: Default::default(),
        };
        save(&project_root, &cfg).expect("seed config");

        let opts = InitProjectOptions {
            slug: Some("team-rust".into()),
            project_uuid: Some(Uuid::now_v7()),
            ..Default::default()
        };
        let err = bootstrap_project(&backend, &groups, &project_root, &opts, false)
            .await
            .expect_err("must reject uuid mismatch");
        assert!(matches!(err, InitProjectError::ProjectUuidMismatch { .. }));
    }

    #[tokio::test]
    async fn init_project_config_only_skips_repo_creation() {
        let (backend, groups, tmp) = test_fixture().await;
        let project_root = tmp.path().join("project");
        std::fs::create_dir_all(&project_root).expect("project root");
        let opts = InitProjectOptions {
            slug: Some("team-rust".into()),
            config_only: true,
            ..Default::default()
        };

        let report = bootstrap_project(&backend, &groups, &project_root, &opts, false)
            .await
            .expect("bootstrap");

        assert!(report.created_config);
        assert!(!report.created_repo, "config-only must never create a repo");
        assert!(
            report.repo_path.is_none(),
            "repo_path must be None when the repo wasn't created",
        );
        assert!(project_root.join(".mmcp.toml").exists());
    }

    #[tokio::test]
    async fn init_project_second_call_after_config_only_creates_repo() {
        let (backend, groups, tmp) = test_fixture().await;
        let project_root = tmp.path().join("project");
        std::fs::create_dir_all(&project_root).expect("project root");

        // First call writes config only, stashes slug.
        let config_only_opts = InitProjectOptions {
            slug: Some("team-rust".into()),
            config_only: true,
            ..Default::default()
        };
        let first = bootstrap_project(&backend, &groups, &project_root, &config_only_opts, false)
            .await
            .expect("first");
        assert!(first.created_config);
        assert!(!first.created_repo);

        // Second call without any args picks the slug from the
        // stored config and creates the repo.
        let follow_up = bootstrap_project(
            &backend,
            &groups,
            &project_root,
            &InitProjectOptions::default(),
            false,
        )
        .await
        .expect("second");
        assert!(!follow_up.created_config);
        assert!(follow_up.created_repo);
        assert_eq!(follow_up.slug, "team-rust");
    }

    #[tokio::test]
    async fn init_project_is_idempotent_when_everything_exists() {
        let (backend, groups, tmp) = test_fixture().await;
        let project_root = tmp.path().join("project");
        std::fs::create_dir_all(&project_root).expect("project root");
        let opts = InitProjectOptions {
            slug: Some("team-rust".into()),
            ..Default::default()
        };
        let _first = bootstrap_project(&backend, &groups, &project_root, &opts, false)
            .await
            .expect("first");
        let second = bootstrap_project(&backend, &groups, &project_root, &opts, false)
            .await
            .expect("second");
        assert!(!second.created_config);
        assert!(!second.created_repo);
    }
}
