//! `mmcp init` implementations.
//!
//! `mmcp init` (no subcommand) writes the per-project `.mmcp.toml`
//! with a freshly generated `project_uuid`. `mmcp init project`
//! creates the git repo that `project_uuid` refers to so memories
//! can be written into it. The two steps are intentionally
//! orthogonal: an operator who is going to sync a pre-existing
//! server-side project down only needs `mmcp init`; an operator
//! starting a brand new local-only project runs both.

use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use inquire::Text;
use mmcp_core::config::{GroupsConfig, LanguagesConfig, ProjectConfig};
use mmcp_core::id::{GroupId, ProjectUuid};
use mmcp_core::manifest::GroupManifest;
use mmcp_git::{GitBackend, NativeBackend};
use thiserror::Error;
use uuid::Uuid;

use crate::commands::import::validate_slug;
use crate::config::{config_path_for, find_project_root, load, save};
use crate::home::MmcpHome;
use crate::state::GroupIndex;

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

// ── `mmcp init project` ─────────────────────────────────────────────

/// Flags accepted by `mmcp init project`.
#[derive(Debug, Clone, clap::Args)]
pub struct ProjectArgs {
    /// Group slug (kebab-case). When omitted, a TTY invocation
    /// prompts for one using the slugified project directory name
    /// as the default; a non-TTY invocation errors so the operator
    /// has to pick a deterministic name up front.
    #[arg(long)]
    pub slug: Option<String>,
}

/// Successful outcome of a project-group initialization.
#[derive(Debug, Clone)]
pub struct ProjectGroupReport {
    pub project_uuid: ProjectUuid,
    pub group_id: GroupId,
    pub slug: String,
    pub repo_path: PathBuf,
    /// `true` when the call actually created the repo; `false` when
    /// it was already present on disk (idempotent path).
    pub created: bool,
}

/// Structured failures emitted by the `init project` code path.
///
/// The MCP tool side maps each variant to a distinct `code` on the
/// wire so AI clients can branch without parsing human text.
#[derive(Debug, Error)]
pub enum InitProjectError {
    #[error(
        "no mmcp project found in current directory or any parent; run `mmcp init` first"
    )]
    ProjectNotFound,

    #[error("failed to load project config: {0}")]
    ConfigLoadFailed(String),

    #[error("invalid slug `{slug}`: must be 1-128 lowercase alphanumeric chars or hyphens, no leading/trailing/consecutive hyphens")]
    InvalidSlug { slug: String },

    #[error("git backend error: {0}")]
    GitBackend(String),

    #[error("group index refresh failed: {0}")]
    IndexRefreshFailed(String),
}

/// CLI entry for `mmcp init project`.
///
/// Handles interactive slug acquisition (TTY prompt with the
/// slugified project dir basename as the default) and pretty-prints
/// the resulting [`ProjectGroupReport`] to stdout.
pub async fn run_project(args: ProjectArgs) -> Result<()> {
    let cwd = std::env::current_dir().context("reading current working directory")?;
    let home = MmcpHome::discover()?;

    let slug = match args.slug {
        Some(s) => s,
        None => resolve_slug_interactive(&cwd)?,
    };

    let report = create_project_group(&home, &cwd, &slug)
        .await
        .map_err(|e| anyhow::anyhow!(e))?;

    if report.created {
        println!(
            "created group {} ({}) at {}",
            report.slug,
            report.group_id,
            report.repo_path.display()
        );
    } else {
        println!(
            "project group {} already exists at {}",
            report.slug,
            report.repo_path.display()
        );
    }
    Ok(())
}

/// Derive a default slug from the project directory basename.
///
/// Split out so it can be unit-tested without spawning a TTY. Falls
/// back to `"project"` when `cwd` has no nameable terminal component
/// (e.g. `/`) so the interactive prompt always has something to
/// offer.
fn default_slug_from_cwd(cwd: &Path) -> String {
    cwd.file_name()
        .and_then(|os| os.to_str())
        .map(slug::slugify)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "project".to_string())
}

/// TTY-bound slug prompt. Non-TTY callers receive an error so CI
/// and scripted runs have to pass `--slug` explicitly instead of
/// hanging on stdin.
fn resolve_slug_interactive(cwd: &Path) -> Result<String> {
    if !std::io::stdin().is_terminal() {
        bail!("--slug required when stdin is not a TTY");
    }
    let default = default_slug_from_cwd(cwd);
    Text::new("Group slug:")
        .with_default(&default)
        .prompt()
        .context("reading group slug from TTY")
}

/// CLI variant: builds a fresh backend + group index from the user's
/// mmcp home, then delegates to the shared inner helper.
///
/// Tests that want to avoid the `MmcpHome::discover` environment
/// cascade can call [`create_project_group_from_state`] directly.
pub async fn create_project_group(
    home: &MmcpHome,
    cwd: &Path,
    slug: &str,
) -> Result<ProjectGroupReport, InitProjectError> {
    let (backend, groups) = home
        .init_backend()
        .await
        .map_err(|e| InitProjectError::GitBackend(e.to_string()))?;
    create_project_group_inner(&backend, &groups, cwd, slug).await
}

/// MCP variant: the server already holds `backend` + `groups` in its
/// `ClientState`, so re-initializing them via `MmcpHome` would both
/// duplicate work and race against the live watcher. This entry
/// point lets the tool method pass the state pieces through.
pub async fn create_project_group_from_state(
    backend: &Arc<NativeBackend>,
    groups: &GroupIndex,
    cwd: &Path,
    slug: &str,
) -> Result<ProjectGroupReport, InitProjectError> {
    create_project_group_inner(backend, groups, cwd, slug).await
}

/// Inner helper shared by both entry points. Idempotent: re-calling
/// against an already-created repo returns `created: false` without
/// writing a new commit.
///
/// Steps:
/// 1. Locate `.mmcp.toml` walking up from `cwd`; error otherwise.
/// 2. Load and parse the config; surface parse errors verbatim.
/// 3. Validate the slug against the shared memory-slug contract so
///    the stored group slug can never disagree with what the import
///    path would accept.
/// 4. Derive `GroupId` from the project's stable UUID.
/// 5. Short-circuit if the repo already exists on disk; otherwise
///    build a fresh manifest (owner = a new v7 UUID, re-generated on
///    each fresh creation — ownership semantics are deferred to the
///    auth track) and invoke `create_group_repo`.
/// 6. Refresh the live `GroupIndex` so subsequent
///    `groups.get(group_id)` calls hit.
async fn create_project_group_inner(
    backend: &Arc<NativeBackend>,
    groups: &GroupIndex,
    cwd: &Path,
    slug: &str,
) -> Result<ProjectGroupReport, InitProjectError> {
    let root = find_project_root(cwd).ok_or(InitProjectError::ProjectNotFound)?;
    let cfg = load(&root).map_err(|e| InitProjectError::ConfigLoadFailed(e.to_string()))?;

    validate_slug(slug).map_err(|_| InitProjectError::InvalidSlug {
        slug: slug.to_string(),
    })?;

    let project_uuid = cfg.project_uuid;
    let group_id = GroupId::from_uuid(*project_uuid.as_uuid());
    let repo_path = backend.repo_path(*project_uuid.as_uuid());

    if repo_path.exists() {
        // The repo is already on disk. Refresh the index so
        // `groups.get(group_id)` is guaranteed to see it even if the
        // caller spun up the index before the watcher noticed the
        // directory — this keeps the idempotent path observable
        // identically to the "just created" path.
        groups
            .refresh()
            .await
            .map_err(|e| InitProjectError::IndexRefreshFailed(e.to_string()))?;
        return Ok(ProjectGroupReport {
            project_uuid,
            group_id,
            slug: slug.to_string(),
            repo_path,
            created: false,
        });
    }

    // Owner is a fresh v7 UUID because we do not yet track a stable
    // user identity in the local home config; auth work will revisit
    // this and backfill. The choice is safe because
    // `create_group_repo` records the owner in the initial manifest
    // commit and never overwrites it afterwards.
    let owner = Uuid::now_v7();
    let manifest = GroupManifest::new_user_owned(group_id, slug.to_string(), owner);
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
        group_id,
        slug: slug.to_string(),
        repo_path,
        created: true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// Assemble a tempdir-backed `(backend, groups, project_root)`
    /// fixture with a pre-written `.mmcp.toml`, so tests can focus
    /// on the creation logic without duplicating plumbing.
    async fn test_fixture() -> (Arc<NativeBackend>, GroupIndex, TempDir, ProjectUuid) {
        let tmp = TempDir::new().expect("tempdir");
        let project_root = tmp.path().to_path_buf();
        let repos_root = tmp.path().join("repos");
        std::fs::create_dir_all(&repos_root).expect("repos root");

        let project_uuid = ProjectUuid::new();
        let cfg = ProjectConfig {
            project_uuid,
            sync: None,
            groups: GroupsConfig::default(),
            languages: LanguagesConfig::default(),
        };
        save(&project_root, &cfg).expect("write .mmcp.toml");

        let backend = Arc::new(NativeBackend::new(&repos_root).expect("backend"));
        let groups = GroupIndex::build(repos_root, backend.clone())
            .await
            .expect("group index");

        (backend, groups, tmp, project_uuid)
    }

    #[test]
    fn default_slug_from_cwd_slugifies_dir_basename() {
        let result = default_slug_from_cwd(Path::new("/srv/My Awesome Project"));
        assert_eq!(result, "my-awesome-project");
    }

    #[test]
    fn default_slug_from_cwd_falls_back_for_unnameable_paths() {
        // Root-like paths (no terminal component) should still
        // produce a usable default so the TTY prompt never starts
        // with an empty string.
        assert_eq!(default_slug_from_cwd(Path::new("/")), "project");
    }

    #[tokio::test]
    async fn create_project_group_errors_when_no_project_config() {
        let tmp = TempDir::new().expect("tempdir");
        let repos_root = tmp.path().join("repos");
        std::fs::create_dir_all(&repos_root).expect("repos root");
        let backend = Arc::new(NativeBackend::new(&repos_root).expect("backend"));
        let groups = GroupIndex::build(repos_root, backend.clone())
            .await
            .expect("group index");

        let err = create_project_group_inner(&backend, &groups, tmp.path(), "any-slug")
            .await
            .expect_err("must fail without .mmcp.toml");
        assert!(matches!(err, InitProjectError::ProjectNotFound));
    }

    #[tokio::test]
    async fn create_project_group_writes_repo_and_refreshes_index() {
        let (backend, groups, tmp, project_uuid) = test_fixture().await;

        let report =
            create_project_group_inner(&backend, &groups, tmp.path(), "team-rust")
                .await
                .expect("create_project_group");

        assert!(report.created, "first call must report created: true");
        assert_eq!(report.slug, "team-rust");
        assert_eq!(report.project_uuid, project_uuid);
        assert!(
            report.repo_path.exists(),
            "repo directory should be on disk after creation",
        );
        assert!(
            groups
                .get(&GroupId::from_uuid(*project_uuid.as_uuid()))
                .await
                .is_some(),
            "GroupIndex must surface the newly created group",
        );
    }

    #[tokio::test]
    async fn create_project_group_is_idempotent_on_second_call() {
        let (backend, groups, tmp, _uuid) = test_fixture().await;

        let first =
            create_project_group_inner(&backend, &groups, tmp.path(), "team-rust")
                .await
                .expect("first");
        assert!(first.created);

        let second =
            create_project_group_inner(&backend, &groups, tmp.path(), "team-rust")
                .await
                .expect("second");
        assert!(!second.created, "second call must report created: false");
        assert_eq!(first.repo_path, second.repo_path);
    }

    #[tokio::test]
    async fn create_project_group_rejects_invalid_slug() {
        let (backend, groups, tmp, _uuid) = test_fixture().await;

        let err = create_project_group_inner(&backend, &groups, tmp.path(), "UPPERCASE")
            .await
            .expect_err("uppercase slug must be rejected");
        assert!(matches!(err, InitProjectError::InvalidSlug { .. }));

        let err2 = create_project_group_inner(&backend, &groups, tmp.path(), "-leading")
            .await
            .expect_err("leading hyphen must be rejected");
        assert!(matches!(err2, InitProjectError::InvalidSlug { .. }));
    }
}
