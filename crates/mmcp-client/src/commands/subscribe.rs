//! CLI surface for `mmcp subscribe` / `mmcp unsubscribe`.
//!
//! Thin adapter over `commands::subscription`, the shared owner of
//! the subscription domain logic (target validation, config
//! mutation, the four subscription axes): this module resolves the
//! project root and local mirror, then calls the same pure mutator
//! the `subscribe` / `unsubscribe` MCP tools call.

use std::path::PathBuf;

use anyhow::{Context, Result, anyhow};
use clap::Args;
use mmcp_store::config::{load, save};
use mmcp_store::home::MmcpHome;

use crate::commands::subscription::{
    SubscriptionAction, SubscriptionKind, apply_subscription, resolve_project_root,
    validate_subscription_target,
};

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
