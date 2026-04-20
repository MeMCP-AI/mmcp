//! `mmcp sync`, `mmcp pull`, and `mmcp push` implementations.
//!
//! All three subcommands share this entry point: the booleans
//! `pull` and `push` toggle which halves of a full sync run. The
//! body calls through to `mmcp_store::sync::build_engine` (the
//! shared wiring used by the MCP tools as well) and formats the
//! resulting report for stdout.

use anyhow::{Context, Result, bail};
use mmcp_core::manifest::GroupScope;
use mmcp_store::config::{find_project_root, load};
use mmcp_store::home::MmcpHome;
use mmcp_store::memory::resolve_group;
use mmcp_store::sync::build_engine;
use mmcp_sync::{SyncError, SyncFilter};

/// CLI-side mirror of `GroupScope` that clap can parse via
/// `ValueEnum`. Kept as a separate wire-form enum so `mmcp-core`
/// stays clap-free; the mapping is a one-line match.
#[derive(Debug, Clone, Copy, clap::ValueEnum)]
#[value(rename_all = "snake_case")]
pub enum ScopeArg {
    Global,
    Shared,
    Project,
}

impl From<ScopeArg> for GroupScope {
    fn from(value: ScopeArg) -> Self {
        match value {
            ScopeArg::Global => GroupScope::Global,
            ScopeArg::Shared => GroupScope::Shared,
            ScopeArg::Project => GroupScope::Project,
        }
    }
}

/// Selector shared by `mmcp sync` / `pull` / `push`.
///
/// The three variants are mutually exclusive (`clap::ArgGroup`
/// enforces it on parse). Empty selector is still accepted for now
/// and falls back to `SyncFilter::All` inside [`resolve_sync_filter`];
/// the breaking flip that rejects empty lands in a follow-up commit
/// so the CLI and MCP sides switch in lockstep.
#[derive(Debug, Clone, clap::Args)]
#[group(id = "sync_selector", multiple = false, required = false)]
pub struct SyncSelector {
    /// Target a single group by UUID or slug.
    #[arg(long, group = "sync_selector")]
    pub group: Option<String>,

    /// Target every locally-known group whose manifest carries the
    /// chosen `GroupScope`.
    #[arg(long, group = "sync_selector", value_enum)]
    pub scope: Option<ScopeArg>,

    /// Explicitly fan out across the whole local mirror. This is
    /// the only way to reproduce the pre-scoping behaviour; the
    /// default is still `--all` until the breaking flip lands.
    #[arg(long, group = "sync_selector", default_value_t = false)]
    pub all: bool,
}

/// Resolve a [`SyncSelector`] into a [`SyncFilter`] against the
/// live group index.
///
/// Empty-selector -> `SyncFilter::All` is a soft default for this
/// commit; the follow-up commit replaces it with a structured
/// `selector_required` error so scripts cannot drift back into the
/// whole-mirror path by accident.
pub async fn resolve_sync_filter(
    selector: &SyncSelector,
    groups: &mmcp_store::GroupIndex,
) -> Result<SyncFilter> {
    if selector.all {
        return Ok(SyncFilter::All);
    }
    if let Some(scope) = selector.scope {
        return Ok(SyncFilter::Scope(scope.into()));
    }
    if let Some(query) = selector.group.as_deref() {
        let entry = resolve_group(groups, query)
            .await
            .with_context(|| format!("resolving group `{query}` for sync selector"))?;
        return Ok(SyncFilter::Group(*entry.manifest.group_id.as_uuid()));
    }
    Ok(SyncFilter::All)
}

/// Run the sync engine.
///
/// `pull` and `push` may be toggled independently so that `mmcp
/// pull` and `mmcp push` reuse the same code path.
///
/// Exit codes (returned via `Result`):
///
/// - Ok(()) - sync succeeded (0).
/// - `anyhow::Error` carrying a `SyncError::Conflict` - conflict,
///   caller maps to exit 2.
/// - Any other `anyhow::Error` - generic failure (1).
pub async fn run(pull: bool, push: bool, selector: SyncSelector) -> Result<()> {
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
    let filter = resolve_sync_filter(&selector, &group_index).await?;
    let (engine, resolver, queue) = build_engine(backend, group_index, &sync_cfg.server_url)?;

    let report = match (pull, push) {
        (true, true) => {
            let report = engine
                .sync(&queue, filter, &resolver, &resolver)
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
            let report = engine
                .pull(filter, &resolver, &resolver)
                .await
                .map_err(to_anyhow)?;
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
            let report = engine
                .push(&queue, filter, &resolver, &resolver)
                .await
                .map_err(to_anyhow)?;
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
