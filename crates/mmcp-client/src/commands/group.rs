//! `create_group` helper for standalone group bootstrap.
//!
//! Unlike [`super::init::create_project_group_from_state`], this
//! surface is not tied to a project directory. It never touches
//! `.mmcp.toml` on the filesystem: it mints a fresh group UUID,
//! builds a [`GroupManifest`] with the caller's scope / display-name
//! / protected flag, commits it into a fresh bare repo under
//! `~/.mmcp/repos/<uuid>.git`, and refreshes the [`GroupIndex`] so
//! subsequent `list_groups` calls see the new group without a
//! restart.
//!
//! Intended callers: operators seeding a `global` or `shared` group
//! (install-wide rule sets, per-language convention bundles,
//! team-wide coding rules) that lives across projects.
//!
//! Slug uniqueness is enforced across the local mirror so callers
//! can always address the group by its slug in downstream tools;
//! a collision surfaces a structured [`CreateGroupError::SlugAlreadyExists`]
//! instead of producing two indistinguishable groups.

use std::path::PathBuf;
use std::sync::Arc;

use mmcp_core::id::{GroupId, UserId};
use mmcp_core::manifest::{GroupManifest, GroupScope};
use mmcp_git::{GitBackend, GitError, NativeBackend};
use thiserror::Error;
use uuid::Uuid;

use mmcp_store::StoreError;
use mmcp_store::groups::GroupIndex;
use mmcp_store::memory::{ImportError, validate_slug_segment};

/// Options accepted by [`create_standalone_group`].
///
/// Kept as a struct rather than a long argument list so every caller
/// converges on the same shape; optional fields default via
/// [`Default`].
#[derive(Debug, Clone)]
pub struct CreateGroupOptions {
    /// Group slug (kebab-case, 1-128 chars). Must satisfy the same
    /// contract as memory slugs. Also checked for uniqueness against
    /// the existing local mirror before any on-disk work starts.
    pub slug: String,

    /// Optional human-readable name used by WebUI and future tool
    /// listings. `None` omits the field from the manifest.
    pub display_name: Option<String>,

    /// Cross-project reach of the new group. `Shared` is the right
    /// default for standalone groups the caller intends to consume
    /// from multiple projects; pick `Global` only for install-wide
    /// rule sets that should surface in every session.
    pub scope: GroupScope,

    /// When true, the manifest's `protected` flag is set so every
    /// subsequent mutation goes through the confirmation
    /// guard.
    pub protected: bool,
}

impl Default for CreateGroupOptions {
    fn default() -> Self {
        Self {
            slug: String::new(),
            display_name: None,
            scope: GroupScope::Shared,
            protected: false,
        }
    }
}

/// Successful outcome of [`create_standalone_group`].
#[derive(Debug, Clone)]
pub struct CreateGroupReport {
    pub group_id: GroupId,
    pub slug: String,
    pub scope: GroupScope,
    pub display_name: Option<String>,
    pub protected: bool,
    pub repo_path: PathBuf,
}

/// Structured failures emitted by [`create_standalone_group`].
///
/// The MCP tool maps each variant to a distinct `code` payload so AI
/// clients branch on state instead of parsing human strings.
#[derive(Debug, Error)]
pub enum CreateGroupError {
    #[error(
        "invalid slug `{slug}`: must be 1-128 lowercase alphanumeric chars or hyphens, no leading/trailing/consecutive hyphens"
    )]
    InvalidSlug { slug: String },

    #[error(
        "a group with slug `{slug}` already exists (group_id {existing_group_id}); pick a different slug or reuse that group"
    )]
    SlugAlreadyExists {
        slug: String,
        existing_group_id: Uuid,
    },

    #[error("git backend error: {0}")]
    GitBackend(String),

    #[error("group index refresh failed: {0}")]
    IndexRefreshFailed(String),
}

/// Create a fresh standalone group repository under the local repos
/// root.
///
/// Mints a UUIDv7 for both the group identity and the owner hint,
/// builds a [`GroupManifest`] from `opts`, and delegates to
/// [`NativeBackend::create_group_repo`] which initializes the bare
/// repo and commits the manifest on `main`. The [`GroupIndex`] is
/// refreshed before returning so any caller that inspects it after
/// the call sees the new group.
pub async fn create_standalone_group(
    backend: &Arc<NativeBackend>,
    groups: &GroupIndex,
    opts: &CreateGroupOptions,
) -> Result<CreateGroupReport, CreateGroupError> {
    validate_slug_segment(&opts.slug).map_err(|_| CreateGroupError::InvalidSlug {
        slug: opts.slug.clone(),
    })?;

    // Group slugs are how operators refer to groups in tool args and
    // manifest migrations, so cross-group uniqueness is a soft
    // contract. Refuse to create a collision so the caller decides
    // between picking a new slug and reusing the existing group.
    for entry in groups.list().await {
        if entry.manifest.slug == opts.slug {
            return Err(CreateGroupError::SlugAlreadyExists {
                slug: opts.slug.clone(),
                existing_group_id: *entry.manifest.group_id.as_uuid(),
            });
        }
    }

    let group_id = GroupId::new();
    // Owner is a v7 UUID. Real ownership semantics remain deferred
    // to the auth track; `create_group_repo` records the owner once
    // and never overwrites it, which matches `init_project`'s shape.
    let owner = UserId::new();
    let mut manifest = GroupManifest::new_user_owned(group_id, opts.slug.clone(), owner);
    manifest.display_name = opts.display_name.clone();
    manifest.scope = opts.scope;
    manifest.protected = opts.protected;

    backend
        .create_group_repo(&manifest)
        .await
        .map_err(|e| CreateGroupError::GitBackend(e.to_string()))?;
    groups
        .refresh()
        .await
        .map_err(|e| CreateGroupError::IndexRefreshFailed(e.to_string()))?;

    let repo_path = backend.repo_path(*group_id.as_uuid());
    Ok(CreateGroupReport {
        group_id,
        slug: opts.slug.clone(),
        scope: opts.scope,
        display_name: opts.display_name.clone(),
        protected: opts.protected,
        repo_path,
    })
}

/// Successful outcome of [`set_group_protected`].
#[derive(Debug, Clone)]
pub struct SetProtectedReport {
    pub group_id: GroupId,
    pub slug: String,
    pub protected: bool,
    pub commit_id: String,
}

/// Structured failures emitted by [`set_group_protected`].
#[derive(Debug, Error)]
pub enum SetProtectedError {
    /// No group in the local mirror matches `identifier`, neither as
    /// a UUID nor as a slug.
    #[error("no group found for identifier `{identifier}`")]
    GroupNotFound {
        identifier: String,
        #[source]
        source: ImportError,
    },

    /// The manifest could not be re-read from the repository's
    /// current tip before flipping `protected`.
    #[error("failed to read group manifest")]
    ReadManifest(#[source] GitError),

    /// The flipped manifest could not be committed back to the
    /// repository.
    #[error("failed to write group manifest")]
    WriteManifest(#[source] GitError),

    /// The in-memory group index could not be refreshed after the
    /// protected flag was committed.
    #[error("group index refresh failed")]
    IndexRefresh(#[source] StoreError),
}

/// Arm or disarm the protected-write guard on an already
/// existing group.
///
/// Resolves `identifier` (UUID or slug) against the local mirror, re-reads
/// the manifest from the repo's current tip, flips `protected`, and
/// commits the result through [`GitBackend::write_manifest`]. The
/// [`GroupIndex`] is refreshed before returning so the protected-
/// write guard itself sees the new value on the very next mutation.
pub async fn set_group_protected(
    backend: &Arc<NativeBackend>,
    groups: &GroupIndex,
    identifier: &str,
    protected: bool,
) -> Result<SetProtectedReport, SetProtectedError> {
    let entry = resolve_group(groups, identifier).await.map_err(|source| {
        SetProtectedError::GroupNotFound {
            identifier: identifier.to_string(),
            source,
        }
    })?;

    let mut manifest = backend
        .read_manifest(&entry.handle)
        .await
        .map_err(SetProtectedError::ReadManifest)?;
    manifest.set_protected(protected);

    let commit_id = backend
        .write_manifest(&entry.handle, &manifest)
        .await
        .map_err(SetProtectedError::WriteManifest)?;

    groups
        .refresh()
        .await
        .map_err(SetProtectedError::IndexRefresh)?;

    Ok(SetProtectedReport {
        group_id: manifest.group_id,
        slug: manifest.slug,
        protected,
        commit_id,
    })
}

// ── CLI surface ─────────────────────────────────────────────────
//
// `mmcp group <verb>` mirrors the `mcp:list_groups`,
// `mcp:group_info`, `mcp:create_group` MCP tools for
// group-management parity with the MCP surface.

use anyhow::{Context, Result};
use clap::{Args, Subcommand};
use mmcp_git::Rev;
use mmcp_store::config::{find_project_root, load as load_project_config};
use mmcp_store::home::MmcpHome;
use mmcp_store::memory::{list_all_memory_files, resolve_group};

#[derive(Debug, Args)]
#[command(arg_required_else_help = true)]
pub struct GroupArgs {
    #[command(subcommand)]
    pub cmd: Option<GroupCommand>,
}

#[derive(Debug, Subcommand)]
pub enum GroupCommand {
    /// List every group present in the local mirror.
    List,
    /// Show manifest metadata for a single group.
    Info(InfoArgs),
    /// Bootstrap a fresh standalone group under ~/.mmcp/repos.
    Create(CreateArgs),
    /// Arm or disarm the protected-write guard on an
    /// already existing group.
    Protect(ProtectArgs),
}

#[derive(Debug, Args)]
pub struct InfoArgs {
    /// Group UUID or slug.
    pub group: String,
}

#[derive(Debug, Args)]
pub struct CreateArgs {
    /// Group slug (kebab-case, 1-128 chars). Must be unique
    /// across the local mirror.
    pub slug: String,

    /// Cross-project reach: `global`, `shared` (default), or
    /// `project`.
    #[arg(long)]
    pub scope: Option<String>,

    /// Optional human-readable display name.
    #[arg(long = "display-name")]
    pub display_name: Option<String>,

    /// Mark the group as protected so every subsequent mutation
    /// goes through the confirmation guard.
    #[arg(long)]
    pub protected: bool,
}

#[derive(Debug, Args)]
pub struct ProtectArgs {
    /// Group UUID or slug.
    pub group: String,

    /// Disarm protection instead of arming it.
    #[arg(long)]
    pub unprotect: bool,
}

pub async fn run(args: GroupArgs) -> Result<()> {
    match args.cmd {
        // `arg_required_else_help` prints help before this branch
        // when no subcommand is supplied; this arm exists to catch
        // future variants added without a dispatch update.
        None => unreachable!("clap enforces subcommand presence"),
        Some(GroupCommand::List) => run_list().await,
        Some(GroupCommand::Info(a)) => run_info(a).await,
        Some(GroupCommand::Create(a)) => run_create(a).await,
        Some(GroupCommand::Protect(a)) => run_protect(a).await,
    }
}

async fn run_list() -> Result<()> {
    let home = MmcpHome::discover()?;
    let (backend, groups) = home.init_backend().await?;

    // Match `mcp:list_groups`'s `is_project` flag by walking cwd
    // for a `.mmcp.toml`. Missing / unreadable config → no row
    // gets flagged.
    let project_uuid = std::env::current_dir()
        .ok()
        .and_then(|cwd| find_project_root(&cwd))
        .and_then(|root| match load_project_config(&root) {
            Ok(cfg) => Some(cfg),
            Err(e) => {
                // Loud, not swallowed (mandatory no-silent-failure
                // rule), but a log line rather than a hard command
                // failure: `mmcp group list`'s own purpose is
                // unrelated to project config health, so failing the
                // whole listing over a broken `is_project` flag
                // would be disproportionate.
                tracing::warn!(
                    path = %root.display(),
                    error = %e,
                    "mmcp group list: failed to load project config; is_project will be false for every row"
                );
                None
            }
        })
        .map(|cfg| *cfg.project_uuid.as_uuid());

    let entries = groups.list().await;
    if entries.is_empty() {
        println!("no groups in the local mirror");
        return Ok(());
    }
    for entry in &entries {
        let files = list_all_memory_files(&backend, &entry.handle, &Rev::head())
            .await
            .context("listing memory files")?;
        let is_project = project_uuid == Some(*entry.manifest.group_id.as_uuid());
        let mut tags: Vec<&'static str> = Vec::new();
        if entry.manifest.protected {
            tags.push("protected");
        }
        if is_project {
            tags.push("project");
        }
        let tag_str = if tags.is_empty() {
            String::new()
        } else {
            format!("  [{}]", tags.join(","))
        };
        println!(
            "{slug} ({uuid})  {count} memor{plural}{tags}",
            slug = entry.manifest.slug,
            uuid = entry.manifest.group_id,
            count = files.len(),
            plural = if files.len() == 1 { "y" } else { "ies" },
            tags = tag_str,
        );
    }
    println!("\n{} group(s)", entries.len());
    Ok(())
}

async fn run_info(args: InfoArgs) -> Result<()> {
    let home = MmcpHome::discover()?;
    let (backend, groups) = home.init_backend().await?;
    let entry = resolve_group(&groups, &args.group)
        .await
        .map_err(anyhow::Error::from)?;
    let files = list_all_memory_files(&backend, &entry.handle, &Rev::head())
        .await
        .context("listing memory files")?;

    println!("group_id       : {}", entry.manifest.group_id);
    println!("slug           : {}", entry.manifest.slug);
    if let Some(name) = &entry.manifest.display_name {
        println!("display_name   : {name}");
    }
    println!("scope          : {}", scope_str(&entry.manifest.scope));
    println!("protected      : {}", entry.manifest.protected);
    println!("schema_version : {}", entry.manifest.schema_version);
    println!("created_at     : {}", entry.manifest.created_at);
    println!("memory_count   : {}", files.len());
    println!(
        "owner          : {} {}",
        owner_kind_str(&entry.manifest.owner),
        owner_id_str(&entry.manifest.owner),
    );
    Ok(())
}

async fn run_create(args: CreateArgs) -> Result<()> {
    let scope = match args.scope.as_deref() {
        None => GroupScope::Shared,
        Some("global") => GroupScope::Global,
        Some("shared") => GroupScope::Shared,
        Some("project") => GroupScope::Project,
        Some(other) => {
            anyhow::bail!("unknown scope `{other}` (expected global / shared / project)")
        }
    };

    let home = MmcpHome::discover()?;
    let (backend, groups) = home.init_backend().await?;
    let opts = CreateGroupOptions {
        slug: args.slug.clone(),
        display_name: args.display_name,
        scope,
        protected: args.protected,
    };
    let report = create_standalone_group(&backend, &groups, &opts)
        .await
        .map_err(anyhow::Error::from)?;
    println!(
        "created group `{}` ({})\n  scope: {}\n  protected: {}\n  repo: {}",
        report.slug,
        report.group_id,
        scope_str(&report.scope),
        report.protected,
        report.repo_path.display(),
    );
    Ok(())
}

async fn run_protect(args: ProtectArgs) -> Result<()> {
    let home = MmcpHome::discover()?;
    let (backend, groups) = home.init_backend().await?;
    let protected = !args.unprotect;
    let report = set_group_protected(&backend, &groups, &args.group, protected)
        .await
        .map_err(anyhow::Error::from)?;
    println!(
        "group `{}` ({})\n  protected: {}\n  commit: {}",
        report.slug, report.group_id, report.protected, report.commit_id,
    );
    Ok(())
}

fn scope_str(s: &GroupScope) -> &'static str {
    match s {
        GroupScope::Global => "global",
        GroupScope::Shared => "shared",
        GroupScope::Project => "project",
    }
}

fn owner_kind_str(owner: &mmcp_core::manifest::GroupOwnerHint) -> &'static str {
    use mmcp_core::manifest::GroupOwnerHint;
    match owner {
        GroupOwnerHint::User(_) => "user",
        GroupOwnerHint::Org(_) => "org",
    }
}

fn owner_id_str(owner: &mmcp_core::manifest::GroupOwnerHint) -> String {
    use mmcp_core::manifest::GroupOwnerHint;
    match owner {
        GroupOwnerHint::User(id) => id.to_string(),
        GroupOwnerHint::Org(id) => id.to_string(),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use mmcp_store::FileOperation;
    use mmcp_store::home::MmcpHome;
    use tempfile::TempDir;

    /// Rooted entirely inside a tempdir so the test never touches
    /// the real user home. Mirrors the fixture pattern used in
    /// `serve.rs` tests.
    async fn scratch_state() -> (TempDir, Arc<NativeBackend>, GroupIndex) {
        let tmp = TempDir::new().expect("tempdir");
        let home = MmcpHome::from_root(tmp.path().join("mmcp-home"));
        let (backend, groups) = home.init_backend().await.expect("init backend");
        (tmp, backend, groups)
    }

    #[tokio::test]
    async fn create_standalone_group_mints_repo_and_refreshes_index() {
        let (_tmp, backend, groups) = scratch_state().await;

        let report = create_standalone_group(
            &backend,
            &groups,
            &CreateGroupOptions {
                slug: "team-rust".into(),
                display_name: Some("Team Rust".into()),
                scope: GroupScope::Shared,
                protected: false,
            },
        )
        .await
        .expect("create group");

        assert_eq!(report.slug, "team-rust");
        assert_eq!(report.scope, GroupScope::Shared);
        assert_eq!(report.display_name.as_deref(), Some("Team Rust"));
        assert!(!report.protected);
        assert!(
            report.repo_path.exists(),
            "bare repo must be on disk at {}",
            report.repo_path.display()
        );

        let listed = groups.list().await;
        assert!(
            listed
                .iter()
                .any(|e| e.manifest.slug == "team-rust" && e.manifest.group_id == report.group_id),
            "GroupIndex must see the newly created group after refresh",
        );
    }

    #[tokio::test]
    async fn create_standalone_group_rejects_duplicate_slug() {
        let (_tmp, backend, groups) = scratch_state().await;

        let first = CreateGroupOptions {
            slug: "shared".into(),
            ..Default::default()
        };
        let first_report = create_standalone_group(&backend, &groups, &first)
            .await
            .expect("first create");

        let err = create_standalone_group(&backend, &groups, &first)
            .await
            .expect_err("second create must error");
        match err {
            CreateGroupError::SlugAlreadyExists {
                slug,
                existing_group_id,
            } => {
                assert_eq!(slug, "shared");
                assert_eq!(existing_group_id, *first_report.group_id.as_uuid());
            }
            other => panic!("expected SlugAlreadyExists, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn create_standalone_group_rejects_invalid_slug() {
        let (_tmp, backend, groups) = scratch_state().await;

        let err = create_standalone_group(
            &backend,
            &groups,
            &CreateGroupOptions {
                slug: "-bad-leading-hyphen".into(),
                ..Default::default()
            },
        )
        .await
        .expect_err("invalid slug must error");

        match &err {
            CreateGroupError::InvalidSlug { slug } => {
                assert_eq!(slug, "-bad-leading-hyphen");
            }
            other => panic!("expected InvalidSlug, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn create_standalone_group_records_scope_and_protection_in_manifest() {
        let (_tmp, backend, groups) = scratch_state().await;

        let report = create_standalone_group(
            &backend,
            &groups,
            &CreateGroupOptions {
                slug: "global-rules".into(),
                display_name: None,
                scope: GroupScope::Global,
                protected: true,
            },
        )
        .await
        .expect("create");

        assert_eq!(report.scope, GroupScope::Global);
        assert!(report.protected);

        let entry = groups
            .list()
            .await
            .into_iter()
            .find(|e| e.manifest.slug == "global-rules")
            .expect("must be indexed");
        assert_eq!(entry.manifest.scope, GroupScope::Global);
        assert!(
            entry.manifest.protected,
            "committed manifest must carry the protected flag"
        );
        assert_eq!(entry.manifest.group_id, report.group_id);
    }

    #[tokio::test]
    async fn set_group_protected_arms_an_existing_unprotected_group() {
        let (_tmp, backend, groups) = scratch_state().await;

        let created = create_standalone_group(
            &backend,
            &groups,
            &CreateGroupOptions {
                slug: "team-rust".into(),
                ..Default::default()
            },
        )
        .await
        .expect("create");
        assert!(!created.protected, "must start out unprotected");

        let report = set_group_protected(&backend, &groups, "team-rust", true)
            .await
            .expect("arm protection");
        assert!(report.protected);
        assert_eq!(report.group_id, created.group_id);

        let entry = groups
            .list()
            .await
            .into_iter()
            .find(|e| e.manifest.group_id == created.group_id)
            .expect("must be indexed");
        assert!(
            entry.manifest.protected,
            "re-read manifest must reflect the arm"
        );
    }

    #[tokio::test]
    async fn set_group_protected_disarms_a_protected_group() {
        let (_tmp, backend, groups) = scratch_state().await;

        let created = create_standalone_group(
            &backend,
            &groups,
            &CreateGroupOptions {
                slug: "global-rules".into(),
                protected: true,
                ..Default::default()
            },
        )
        .await
        .expect("create");
        assert!(created.protected);

        let report = set_group_protected(&backend, &groups, &created.group_id.to_string(), false)
            .await
            .expect("disarm protection");
        assert!(!report.protected);

        let entry = groups
            .list()
            .await
            .into_iter()
            .find(|e| e.manifest.group_id == created.group_id)
            .expect("must be indexed");
        assert!(
            !entry.manifest.protected,
            "re-read manifest must reflect the disarm"
        );
    }

    #[tokio::test]
    async fn set_group_protected_rejects_unknown_group() {
        let (_tmp, backend, groups) = scratch_state().await;

        let err = set_group_protected(&backend, &groups, "does-not-exist", true)
            .await
            .expect_err("unknown group must error");
        match &err {
            SetProtectedError::GroupNotFound { identifier, source } => {
                assert_eq!(identifier, "does-not-exist");
                assert!(
                    matches!(source, ImportError::GroupNotFound(_)),
                    "source must be the real resolve_group failure, got {source:?}"
                );
            }
            other => panic!("expected GroupNotFound, got {other:?}"),
        }
        // The source chain must be walkable via `std::error::Error`,
        // not just accessible through the enum's own field.
        let source = std::error::Error::source(&err).expect("must chain a source");
        assert!(
            source.downcast_ref::<ImportError>().is_some(),
            "chained source must downcast to the real ImportError, not a stringified copy"
        );
    }

    #[test]
    fn set_protected_error_read_manifest_preserves_the_git_source_chain() {
        let source = GitError::RepoNotFound("missing.git".into());
        let err = SetProtectedError::ReadManifest(source);

        let chained = std::error::Error::source(&err)
            .and_then(|s| s.downcast_ref::<GitError>())
            .expect("ReadManifest must chain the real GitError, not a stringified copy");
        assert!(matches!(chained, GitError::RepoNotFound(_)));
    }

    #[test]
    fn set_protected_error_write_manifest_preserves_the_git_source_chain() {
        let source = GitError::Unsupported("push");
        let err = SetProtectedError::WriteManifest(source);

        let chained = std::error::Error::source(&err)
            .and_then(|s| s.downcast_ref::<GitError>())
            .expect("WriteManifest must chain the real GitError, not a stringified copy");
        assert!(matches!(chained, GitError::Unsupported(_)));
    }

    #[test]
    fn set_protected_error_index_refresh_preserves_the_store_source_chain() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "repos root vanished");
        let err = SetProtectedError::IndexRefresh(StoreError::Io {
            path: PathBuf::from("/mmcp-home/repos"),
            operation: FileOperation::ReadDir,
            source: io_err,
        });

        let chained = std::error::Error::source(&err)
            .and_then(|s| s.downcast_ref::<StoreError>())
            .expect("IndexRefresh must chain the real StoreError, not a stringified copy");
        assert!(matches!(chained, StoreError::Io { .. }));
    }
}
