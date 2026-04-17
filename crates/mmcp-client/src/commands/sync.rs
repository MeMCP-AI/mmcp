//! `mmcp sync`, `mmcp pull`, and `mmcp push` implementations.
//!
//! All three subcommands share this entry point: the booleans
//! `pull` and `push` toggle which halves of a full sync run. The
//! body calls through to `mmcp_store::sync::build_engine` (the
//! shared wiring used by the MCP tools as well) and formats the
//! resulting report for stdout.
//!
//! `build_engine` and `IndexResolver` now live in
//! `mmcp-store::sync`; the `pub(crate) use` below keeps the
//! `crate::commands::sync::build_engine` import path working for
//! `commands/serve.rs` until commit 8 deletes the shim layer.

use anyhow::{Context, Result, bail};
use mmcp_sync::SyncError;

use crate::config::{find_project_root, load};
use crate::home::MmcpHome;

pub(crate) use mmcp_store::sync::build_engine;

/// Run the sync engine.
///
/// `pull` and `push` may be toggled independently so that `mmcp
/// pull` and `mmcp push` reuse the same code path.
///
/// Exit codes (returned via `Result`):
///
/// - Ok(()) — sync succeeded (0).
/// - `anyhow::Error` carrying a `SyncError::Conflict` — conflict,
///   caller maps to exit 2.
/// - Any other `anyhow::Error` — generic failure (1).
pub async fn run(pull: bool, push: bool) -> Result<()> {
    let cwd = std::env::current_dir().context("reading current working directory")?;
    let root = find_project_root(&cwd)
        .context("no mmcp project found in current directory or any parent")?;
    let cfg = load(&root)?;

    let Some(sync_cfg) = cfg.sync.as_ref() else {
        bail!(
            "project {} has no [sync] block; cannot sync against a remote",
            cfg.project_uuid
        );
    };

    let mmcp_home = MmcpHome::discover()?;
    let (backend, group_index) = mmcp_home.init_backend().await?;
    let (engine, resolver, queue) = build_engine(backend, group_index, &sync_cfg.server_url)?;

    let report = match (pull, push) {
        (true, true) => {
            let report = engine
                .sync(&queue, &resolver)
                .await
                .map_err(to_anyhow)?;
            tracing::info!(
                server = %sync_cfg.server_url,
                updated = report.pulled.updated.len(),
                new_groups = report.pulled.new_groups.len(),
                drained = report.pushed.drained.len(),
                "sync completed"
            );
            format!(
                "sync against {} completed: pulled {} groups ({} new), pushed {} edits",
                sync_cfg.server_url,
                report.pulled.updated.len(),
                report.pulled.new_groups.len(),
                report.pushed.drained.len()
            )
        }
        (true, false) => {
            let report = engine.pull(&resolver).await.map_err(to_anyhow)?;
            tracing::info!(
                server = %sync_cfg.server_url,
                updated = report.updated.len(),
                new_groups = report.new_groups.len(),
                "pull completed"
            );
            format!(
                "pull from {} completed: {} groups updated, {} new groups",
                sync_cfg.server_url,
                report.updated.len(),
                report.new_groups.len()
            )
        }
        (false, true) => {
            let report = engine.push(&queue, &resolver).await.map_err(to_anyhow)?;
            tracing::info!(
                server = %sync_cfg.server_url,
                drained = report.drained.len(),
                "push completed"
            );
            format!(
                "push to {} completed: {} edits drained",
                sync_cfg.server_url,
                report.drained.len()
            )
        }
        (false, false) => {
            bail!("neither pull nor push requested; nothing to do");
        }
    };

    println!("{report}");
    Ok(())
}

fn to_anyhow(err: SyncError) -> anyhow::Error {
    anyhow::Error::from(err)
}

