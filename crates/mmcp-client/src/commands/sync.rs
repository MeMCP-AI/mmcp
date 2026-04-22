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
/// Exactly one of `--group` / `--scope` / `--all` is required;
/// clap's `ArgGroup(required = true, multiple = false)` enforces
/// both at parse time so bare `mmcp sync` fails with a
/// user-visible error message instead of silently operating on
/// the whole mirror.
#[derive(Debug, Clone, clap::Args)]
#[group(id = "sync_selector", multiple = false, required = true)]
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
/// Reaches the empty-selector fallback only if clap's required
/// ArgGroup were bypassed (for example, an internal caller that
/// constructs a `SyncSelector` by hand). That path bails with a
/// user-visible error so the fallback never silently fans out.
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
    bail!(
        "sync selector required: pass exactly one of `--group <uuid|slug>`, `--scope <global|shared|project>`, or `--all`"
    );
}

/// Run the read-only `mmcp fetch` subcommand.
///
/// Walks the in-scope groups and writes each remote head into the
/// local remote-tracking ref without advancing `refs/heads/main`.
/// Mirrors `git fetch` semantics so operators can inspect what
/// would land before `mmcp pull` fast-forwards.
pub async fn run_fetch(selector: SyncSelector) -> Result<()> {
    let cwd = std::env::current_dir().context("reading current working directory")?;
    let root = find_project_root(&cwd)
        .context("no mmcp project found in current directory or any parent")?;
    let cfg = load(&root)?;
    let Some(sync_cfg) = cfg.sync.as_ref() else {
        bail!(
            "project {} has no [sync] block; cannot fetch against a remote",
            cfg.project_uuid
        );
    };

    let mmcp_home = MmcpHome::discover()?;
    let (backend, group_index) = mmcp_home.init_backend().await?;
    let filter = resolve_sync_filter(&selector, &group_index).await?;
    let (engine, resolver) = build_engine(backend, group_index, &sync_cfg.server_url)?;
    let report = engine
        .fetch(filter, &resolver, &resolver)
        .await
        .map_err(to_anyhow)?;
    tracing::info!(
        server = %sync_cfg.server_url,
        groups = report.groups.len(),
        new_groups = report.new_groups.len(),
        "fetch completed"
    );
    println!(
        "fetch from {} completed: {} groups tracked, {} new groups advertised",
        sync_cfg.server_url,
        report.groups.len(),
        report.new_groups.len()
    );
    Ok(())
}

/// Shared prelude for every sync verb: resolve the project, the
/// sync config, the backend, and the requested [`SyncFilter`] so
/// the verb-specific runner just calls the matching engine method.
///
/// Keeping this factored out stops each runner from re-implementing
/// the same eight lines of boilerplate and guarantees `mmcp fetch`,
/// `mmcp pull`, `mmcp push`, and `mmcp sync` all see identical
/// project discovery / config loading / error wording.
async fn prepare(
    selector: &SyncSelector,
) -> Result<(
    String,
    mmcp_sync::SyncEngine,
    mmcp_store::sync::IndexResolver,
    mmcp_sync::SyncFilter,
)> {
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
    let server_url = sync_cfg.server_url.clone();

    let mmcp_home = MmcpHome::discover()?;
    let (backend, group_index) = mmcp_home.init_backend().await?;
    let filter = resolve_sync_filter(selector, &group_index).await?;
    let (engine, resolver) = build_engine(backend, group_index, &server_url)?;
    Ok((server_url, engine, resolver, filter))
}

/// Run `mmcp pull`: fetch each in-scope group's remote head into
/// the local tracking ref, then fast-forward local `main`.
pub async fn run_pull(selector: SyncSelector) -> Result<()> {
    let (server_url, engine, resolver, filter) = prepare(&selector).await?;
    let report = engine
        .pull(filter, &resolver, &resolver)
        .await
        .map_err(to_anyhow)?;
    tracing::info!(
        server = %server_url,
        updated = report.updated.len(),
        new_groups = report.new_groups.len(),
        "pull completed"
    );
    println!(
        "pull from {} completed: {} groups updated, {} new groups",
        server_url,
        report.updated.len(),
        report.new_groups.len()
    );
    Ok(())
}

/// Run `mmcp push`: walk each in-scope group and ship local `main`
/// to the remote.
pub async fn run_push(selector: SyncSelector) -> Result<()> {
    let (server_url, engine, resolver, filter) = prepare(&selector).await?;
    let report = engine
        .push(filter, &resolver, &resolver)
        .await
        .map_err(to_anyhow)?;
    tracing::info!(
        server = %server_url,
        pushed = report.pushed.len(),
        "push completed"
    );
    println!(
        "push to {} completed: {} groups pushed",
        server_url,
        report.pushed.len()
    );
    Ok(())
}

/// Run `mmcp sync`: pull then push against the same filter.
///
/// Exit-code contract (returned via `Result`): `Ok(())` on success,
/// an `anyhow::Error` carrying `SyncError::Conflict` on conflict
/// (caller maps to exit 2), any other `anyhow::Error` generic (1).
pub async fn run_sync(selector: SyncSelector) -> Result<()> {
    let (server_url, engine, resolver, filter) = prepare(&selector).await?;
    let report = engine
        .sync(filter, &resolver, &resolver)
        .await
        .map_err(to_anyhow)?;
    tracing::info!(
        server = %server_url,
        updated = report.pulled.updated.len(),
        new_groups = report.pulled.new_groups.len(),
        pushed = report.pushed.pushed.len(),
        "sync completed"
    );
    println!(
        "sync against {} completed: pulled {} groups ({} new), pushed {} groups",
        server_url,
        report.pulled.updated.len(),
        report.pulled.new_groups.len(),
        report.pushed.pushed.len()
    );
    Ok(())
}

fn to_anyhow(err: SyncError) -> anyhow::Error {
    anyhow::Error::from(err)
}
