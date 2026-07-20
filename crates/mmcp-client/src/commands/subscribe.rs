//! Shared subscribe / unsubscribe surface for `.mmcp/config.toml`.
//!
//! One module powers four call sites: the `subscribe` MCP tool, the
//! `unsubscribe` MCP tool, the `mmcp subscribe` CLI subcommand, and
//! the `mmcp unsubscribe` CLI subcommand. The CLI and MCP layers do
//! the same validation against the local mirror, then call the pure
//! mutator below.
//!
//! The four subscription axes mirror `SubscriptionsConfig`:
//!
//! - `tag`: free-form, no validation; tags don't have to exist yet.
//! - `language`: free-form (resolves to `lang/<value>` later); the
//!   group does not have to be mirrored locally yet.
//! - `group`: must resolve to a group present in the local mirror.
//! - `memory`: `<group_uuid>:<slug>`; both halves must resolve.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use clap::{Args, ValueEnum};
use mmcp_core::config::ProjectConfig;
use mmcp_core::id::GroupId;
use mmcp_git::{NativeBackend, Rev};
use mmcp_store::GroupIndex;
use mmcp_store::config::{find_project_root, load, save};
use mmcp_store::home::MmcpHome;
use mmcp_store::memory::{list_all_memory_files, resolve_group};
use rmcp::schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// The four subscription axes. Each maps to a `Vec<String>` field
/// inside `SubscriptionsConfig`.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize, Serialize, ValueEnum, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
#[clap(rename_all = "snake_case")]
#[schemars(crate = "rmcp::schemars")]
pub enum SubscriptionKind {
    /// Free-form tag filter. Non-mandatory memories from in-scope
    /// groups whose frontmatter tags overlap the subscribed set
    /// surface in `bootstrap_context`.
    Tag,
    /// Individual memory pin, formatted `<group_uuid>:<slug>`.
    Memory,
    /// Whole-group subscription. Pulls every memory (mandatory or
    /// not) from the named group into scope.
    Group,
    /// Language convention group. Resolves to `lang/<name>`.
    Language,
}

impl SubscriptionKind {
    /// Wire-form name as it appears in JSON / CLI args.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Tag => "tag",
            Self::Memory => "memory",
            Self::Group => "group",
            Self::Language => "language",
        }
    }
}

/// Whether a request adds an entry or removes one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubscriptionAction {
    Subscribe,
    Unsubscribe,
}

impl SubscriptionAction {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Subscribe => "subscribe",
            Self::Unsubscribe => "unsubscribe",
        }
    }
}

/// Validation errors for the subscribe/unsubscribe surface.
#[derive(Debug, thiserror::Error)]
pub enum SubscribeError {
    #[error("not in an mmcp project; no .mmcp.toml found at or above the path")]
    NotInProject,
    #[error("memory value must be `<group_uuid>:<slug>`; got: {0}")]
    MalformedMemoryValue(String),
    #[error("memory group uuid is not a valid UUID: {0}")]
    InvalidGroupUuid(String),
    #[error("group `{0}` does not exist in the local mirror")]
    UnknownGroup(String),
    #[error("memory `{slug}` does not exist in group `{group}`")]
    UnknownMemory { group: String, slug: String },
}

/// Pure mutation against `ProjectConfig`. Returns whether anything
/// actually changed (so the caller can avoid a no-op write).
pub fn apply_subscription(
    cfg: &mut ProjectConfig,
    kind: SubscriptionKind,
    value: &str,
    action: SubscriptionAction,
) -> bool {
    let bucket: &mut Vec<String> = match kind {
        SubscriptionKind::Tag => &mut cfg.subscriptions.tags,
        SubscriptionKind::Memory => &mut cfg.subscriptions.memories,
        SubscriptionKind::Group => &mut cfg.subscriptions.groups,
        SubscriptionKind::Language => &mut cfg.subscriptions.languages,
    };
    match action {
        SubscriptionAction::Subscribe => {
            if bucket.iter().any(|v| v == value) {
                false
            } else {
                bucket.push(value.to_string());
                true
            }
        }
        SubscriptionAction::Unsubscribe => {
            let before = bucket.len();
            bucket.retain(|v| v != value);
            before != bucket.len()
        }
    }
}

/// Validate that a subscription target exists in the local mirror.
///
/// `tag` and `language` skip validation — neither needs to exist
/// yet. `group` resolves the value (UUID or slug) against the local
/// mirror. `memory` parses `<group_uuid>:<slug>` and verifies both
/// halves resolve.
pub async fn validate_subscription_target(
    backend: &NativeBackend,
    groups: &GroupIndex,
    kind: SubscriptionKind,
    value: &str,
) -> Result<(), SubscribeError> {
    match kind {
        SubscriptionKind::Tag | SubscriptionKind::Language => Ok(()),
        SubscriptionKind::Group => resolve_group(groups, value)
            .await
            .map(|_| ())
            .map_err(|_| SubscribeError::UnknownGroup(value.to_string())),
        SubscriptionKind::Memory => {
            let (group_part, slug_part) = value
                .split_once(':')
                .ok_or_else(|| SubscribeError::MalformedMemoryValue(value.to_string()))?;
            if slug_part.is_empty() {
                return Err(SubscribeError::MalformedMemoryValue(value.to_string()));
            }
            let group_uuid = Uuid::parse_str(group_part)
                .map_err(|_| SubscribeError::InvalidGroupUuid(group_part.to_string()))?;
            let group_id = GroupId::from_uuid(group_uuid);
            let entry = groups
                .get(&group_id)
                .await
                .ok_or_else(|| SubscribeError::UnknownGroup(group_part.to_string()))?;
            let files = list_all_memory_files(backend, &entry.handle, &Rev::head())
                .await
                .map_err(|_| SubscribeError::UnknownGroup(group_part.to_string()))?;
            if files.iter().any(|f| f.slug == slug_part) {
                Ok(())
            } else {
                Err(SubscribeError::UnknownMemory {
                    group: group_part.to_string(),
                    slug: slug_part.to_string(),
                })
            }
        }
    }
}

/// Resolve the project root from an optional override or the cwd
/// walk. Mirrors the discovery used by every other project-scoped
/// command.
pub fn resolve_project_root(
    explicit: Option<&Path>,
    cwd: Option<&Path>,
) -> Result<PathBuf, SubscribeError> {
    if let Some(path) = explicit {
        if path.join(mmcp_store::config::PROJECT_MANIFEST).exists() {
            return Ok(path.to_path_buf());
        }
        return Err(SubscribeError::NotInProject);
    }
    let cwd = cwd.ok_or(SubscribeError::NotInProject)?;
    find_project_root(cwd).ok_or(SubscribeError::NotInProject)
}

/// MCP wire form shared by `subscribe` and `unsubscribe`.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct SubscribeMcpArgs {
    /// Which subscription axis to mutate.
    pub kind: SubscriptionKind,

    /// Target value. For `kind=tag`, free-form. For `kind=memory`,
    /// `<group_uuid>:<slug>`. For `kind=group`, a UUID or slug
    /// resolvable in the local mirror. For `kind=language`, the
    /// bare language name (resolves to `lang/<name>`).
    pub value: String,

    /// Optional project root override. Defaults to walking the
    /// server's cwd for `.mmcp.toml`. The MCP server is normally
    /// launched inside the project root, so most callers leave this
    /// unset.
    #[serde(default)]
    pub path: Option<String>,
}

/// CLI args shared by `mmcp subscribe` and `mmcp unsubscribe`.
#[derive(Debug, Args)]
pub struct SubscribeCliArgs {
    /// Which subscription axis to mutate.
    #[arg(value_enum)]
    pub kind: SubscriptionKind,

    /// Target value. For `kind=tag`, free-form. For
    /// `kind=memory`, `<group_uuid>:<slug>`. For `kind=group`,
    /// a UUID or slug. For `kind=language`, the bare language name
    /// (resolves to `lang/<name>`).
    pub value: String,

    /// Project root override. Defaults to walking cwd for
    /// `.mmcp.toml`.
    #[arg(long)]
    pub path: Option<PathBuf>,
}

/// CLI entry for `mmcp subscribe`. Validates, mutates,
/// and persists the project config.
pub async fn run_subscribe(args: SubscribeCliArgs) -> Result<()> {
    run_cli(args, SubscriptionAction::Subscribe).await
}

/// CLI entry for `mmcp unsubscribe`. Same path as `run_subscribe`
/// with the action flipped.
pub async fn run_unsubscribe(args: SubscribeCliArgs) -> Result<()> {
    run_cli(args, SubscriptionAction::Unsubscribe).await
}

async fn run_cli(args: SubscribeCliArgs, action: SubscriptionAction) -> Result<()> {
    let cwd = std::env::current_dir().context("reading current working directory")?;
    let project_root =
        resolve_project_root(args.path.as_deref(), Some(&cwd)).map_err(|e| anyhow!("{e}"))?;

    let home = MmcpHome::discover()?;
    let (backend, groups) = home.init_backend().await?;

    validate_subscription_target(&backend, &groups, args.kind, &args.value)
        .await
        .map_err(|e| anyhow!("{e}"))?;

    let mut cfg = load(&project_root)?;
    let changed = apply_subscription(&mut cfg, args.kind, &args.value, action);
    if changed {
        save(&project_root, &cfg)?;
        println!("{} {} {}", action.as_str(), args.kind.as_str(), args.value);
    } else {
        let already = match action {
            SubscriptionAction::Subscribe => "already subscribed",
            SubscriptionAction::Unsubscribe => "not subscribed",
        };
        println!("noop ({}): {} {}", already, args.kind.as_str(), args.value);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use mmcp_core::config::SubscriptionsConfig;
    use mmcp_core::id::ProjectUuid;

    fn cfg() -> ProjectConfig {
        ProjectConfig {
            project_uuid: ProjectUuid::new(),
            project_slug: None,
            sync: None,
            subscriptions: SubscriptionsConfig::default(),
        }
    }

    #[test]
    fn subscribe_tag_appends_unique() {
        let mut c = cfg();
        assert!(apply_subscription(
            &mut c,
            SubscriptionKind::Tag,
            "rust",
            SubscriptionAction::Subscribe,
        ));
        assert_eq!(c.subscriptions.tags, vec!["rust".to_string()]);
        // Re-apply is a no-op.
        assert!(!apply_subscription(
            &mut c,
            SubscriptionKind::Tag,
            "rust",
            SubscriptionAction::Subscribe,
        ));
        assert_eq!(c.subscriptions.tags.len(), 1);
    }

    #[test]
    fn unsubscribe_missing_is_noop() {
        let mut c = cfg();
        assert!(!apply_subscription(
            &mut c,
            SubscriptionKind::Memory,
            "deadbeef:slug",
            SubscriptionAction::Unsubscribe,
        ));
    }

    #[test]
    fn each_kind_targets_its_own_bucket() {
        let mut c = cfg();
        apply_subscription(
            &mut c,
            SubscriptionKind::Tag,
            "git",
            SubscriptionAction::Subscribe,
        );
        apply_subscription(
            &mut c,
            SubscriptionKind::Memory,
            "uuid:slug",
            SubscriptionAction::Subscribe,
        );
        apply_subscription(
            &mut c,
            SubscriptionKind::Group,
            "team/shared",
            SubscriptionAction::Subscribe,
        );
        apply_subscription(
            &mut c,
            SubscriptionKind::Language,
            "rust",
            SubscriptionAction::Subscribe,
        );
        assert_eq!(c.subscriptions.tags, vec!["git".to_string()]);
        assert_eq!(c.subscriptions.memories, vec!["uuid:slug".to_string()]);
        assert_eq!(c.subscriptions.groups, vec!["team/shared".to_string()]);
        assert_eq!(c.subscriptions.languages, vec!["rust".to_string()]);
    }

    #[test]
    fn round_trip_subscribe_then_unsubscribe() {
        let mut c = cfg();
        apply_subscription(
            &mut c,
            SubscriptionKind::Tag,
            "rust",
            SubscriptionAction::Subscribe,
        );
        assert!(apply_subscription(
            &mut c,
            SubscriptionKind::Tag,
            "rust",
            SubscriptionAction::Unsubscribe,
        ));
        assert!(c.subscriptions.tags.is_empty());
    }
}
