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

use mmcp_core::id::GroupId;
use mmcp_core::manifest::{GroupManifest, GroupScope};
use mmcp_git::{GitBackend, NativeBackend};
use thiserror::Error;
use uuid::Uuid;

use mmcp_store::groups::GroupIndex;
use mmcp_store::memory::validate_slug;

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
    /// subsequent mutation goes through the FR-019 confirmation
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
    validate_slug(&opts.slug).map_err(|_| CreateGroupError::InvalidSlug {
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
    let owner = Uuid::now_v7();
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

#[cfg(test)]
mod tests {
    use super::*;
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
}
