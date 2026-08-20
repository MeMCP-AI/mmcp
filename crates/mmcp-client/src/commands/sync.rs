//! `mmcp sync`, `mmcp pull`, and `mmcp push` implementations.
//!
//! All three subcommands share this entry point: the booleans
//! `pull` and `push` toggle which halves of a full sync run. The
//! body calls through to `mmcp_store::sync::build_engine` (the
//! shared wiring used by the MCP tools as well) and formats the
//! resulting report for stdout.
//!
//! Every sync verb resolves against the EFFECTIVE remote set (user
//! config plus project config, merged per FR-301's precedence rules
//! by `mmcp_store::resolve_effective_remotes`), not a single
//! `server_url`: a project may configure zero, one, or several
//! remotes at either level. `push` targets `PushScope::Default`
//! (the resolved default remote only) at every call site here;
//! `--all-remotes` / `--remote <name>` CLI flags for the other
//! `PushScope` variants are a later wave's job, not this one's.

use anyhow::{Context, Result, bail};
use mmcp_core::manifest::GroupScope;
use mmcp_store::config::{find_project_root, load};
use mmcp_store::home::MmcpHome;
use mmcp_store::memory::resolve_group;
use mmcp_store::sync::build_engine;
use mmcp_store::{EffectiveRemotes, resolve_effective_remotes};
use mmcp_sync::{PushScope, SyncError, SyncFilter};

use crate::notes::{render_notes_tail, sync_group_failure_notes, sync_push_partial_failure_notes};

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

/// Short human-readable label for a resolved remote set, used in
/// place of the old single `server_url` in log lines and notes
/// context. Wave 3 (`mmcp status` / `--all-remotes` display) owns a
/// real remotes listing; this stays a terse summary. `pub(crate)` so
/// the MCP `sync_*` tool handlers in `commands::serve` share the
/// exact same label instead of drifting onto their own wording.
pub(crate) fn remotes_label(effective: &EffectiveRemotes) -> String {
    match effective.default_remote() {
        Some(default) if effective.remotes.len() == 1 => default.name().to_string(),
        Some(default) => format!(
            "{} remote(s), default '{}'",
            effective.remotes.len(),
            default.name()
        ),
        None => "(no remotes configured)".to_string(),
    }
}

/// Load the project + user config, resolve the effective remote set,
/// and bail loudly when it is empty: every sync verb needs at least
/// one remote to do anything.
async fn load_effective_remotes(root: &std::path::Path) -> Result<EffectiveRemotes> {
    let project_cfg = load(root)?;
    let mmcp_home = MmcpHome::discover()?;
    let user_cfg = mmcp_home.load_user_config()?;
    let effective = resolve_effective_remotes(&user_cfg, &project_cfg)?;
    if effective.remotes.is_empty() {
        bail!(
            "project {} has no sync remotes configured, in [sync] at user or project level; cannot sync",
            project_cfg.project_uuid
        );
    }
    Ok(effective)
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
    let effective = load_effective_remotes(&root).await?;
    let label = remotes_label(&effective);

    let mmcp_home = MmcpHome::discover()?;
    let (backend, group_index) = mmcp_home.init_backend().await?;
    let filter = resolve_sync_filter(&selector, &group_index).await?;
    let (engine, resolver) = build_engine(backend, group_index, &effective).await?;
    let report = engine
        .fetch(filter, &resolver, &resolver)
        .await
        .map_err(to_anyhow)?;
    tracing::info!(
        remotes = %label,
        groups = report.groups.len(),
        new_groups = report.new_groups.len(),
        failed = report.failed.len(),
        "fetch completed"
    );
    println!(
        "fetch from {} completed: {} groups tracked, {} new groups advertised, {} groups failed",
        label,
        report.groups.len(),
        report.new_groups.len(),
        report.failed.len()
    );
    render_notes_tail(&sync_group_failure_notes(&report.failed, "fetch", &label));
    // A3 partial-success fix: an earlier-in-order group's failure no
    // longer discards a later group's success (see
    // `mmcp_sync::engine::GroupSyncFailure`'s doc comment), but the
    // command must still exit non-zero when any group failed, or a
    // real failure would silently read as success.
    if !report.failed.is_empty() {
        bail!(
            "fetch from {} failed for {} of {} groups; see notes above for per-group errors",
            label,
            report.failed.len(),
            report.failed.len() + report.groups.len()
        );
    }
    Ok(())
}

/// Shared prelude for every sync verb: resolve the project, the
/// effective remote set, the backend, and the requested
/// [`SyncFilter`] so the verb-specific runner just calls the
/// matching engine method.
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
    SyncFilter,
    std::sync::Arc<mmcp_git::NativeBackend>,
)> {
    let cwd = std::env::current_dir().context("reading current working directory")?;
    let root = find_project_root(&cwd)
        .context("no mmcp project found in current directory or any parent")?;
    let effective = load_effective_remotes(&root).await?;
    let label = remotes_label(&effective);

    let mmcp_home = MmcpHome::discover()?;
    let (backend, group_index) = mmcp_home.init_backend().await?;
    let filter = resolve_sync_filter(selector, &group_index).await?;
    // Keep our own handle on the backend for the pull-trigger cache
    // hook below; `build_engine` takes ownership of a clone.
    let backend_for_cache = backend.clone();
    let (engine, resolver) = build_engine(backend, group_index, &effective).await?;
    Ok((label, engine, resolver, filter, backend_for_cache))
}

/// Run `mmcp pull`: fetch each in-scope group's remote head into
/// the local tracking ref, then fast-forward local `main` from the
/// default remote only.
pub async fn run_pull(selector: SyncSelector) -> Result<()> {
    let (label, engine, resolver, filter, backend) = prepare(&selector).await?;
    let report = engine
        .pull(filter, &resolver, &resolver)
        .await
        .map_err(to_anyhow)?;
    notify_cache_of_pull(&backend, &resolver.index, &report).await;
    tracing::info!(
        remotes = %label,
        updated = report.updated.len(),
        new_groups = report.new_groups.len(),
        failed = report.failed.len(),
        "pull completed"
    );
    println!(
        "pull from {} completed: {} groups updated, {} new groups, {} groups failed",
        label,
        report.updated.len(),
        report.new_groups.len(),
        report.failed.len()
    );
    render_notes_tail(&sync_group_failure_notes(&report.failed, "pull", &label));
    if !report.failed.is_empty() {
        bail!(
            "pull from {} failed for {} of {} groups; see notes above for per-group errors",
            label,
            report.failed.len(),
            report.failed.len() + report.updated.len()
        );
    }
    Ok(())
}

/// Run `mmcp push`: walk each in-scope group and ship local `main`
/// to the default remote. `--all-remotes` / `--remote <name>` are a
/// later wave's job; every push here runs with `PushScope::Default`.
pub async fn run_push(selector: SyncSelector) -> Result<()> {
    let (label, engine, resolver, filter, _backend) = prepare(&selector).await?;
    let report = engine
        .push(filter, PushScope::Default, &resolver, &resolver)
        .await
        .map_err(to_anyhow)?;
    tracing::info!(
        remotes = %label,
        pushed = report.total_pushed(),
        failed = report.total_failed(),
        "push completed"
    );
    println!(
        "push to {} completed: {} groups pushed, {} groups failed",
        label,
        report.total_pushed(),
        report.total_failed()
    );
    let mut notes = sync_push_partial_failure_notes(&report);
    for (remote_name, failures) in report
        .by_remote
        .iter()
        .map(|outcome| (outcome.remote_name.as_str(), outcome.failed.as_slice()))
    {
        notes.extend(sync_group_failure_notes(failures, "push", remote_name));
    }
    render_notes_tail(&notes);
    if report.total_failed() > 0 {
        bail!(
            "push to {} failed for {} of {} groups; see notes above for per-group errors",
            label,
            report.total_failed(),
            report.total_failed() + report.total_pushed()
        );
    }
    Ok(())
}

/// Run `mmcp sync`: pull then push against the same filter, push
/// scoped to the default remote only.
///
/// Exit-code contract (returned via `Result`): `Ok(())` on success,
/// an `anyhow::Error` carrying `SyncError::Conflict` on conflict
/// (caller maps to exit 2), any other `anyhow::Error` generic (1).
pub async fn run_sync(selector: SyncSelector) -> Result<()> {
    let (label, engine, resolver, filter, backend) = prepare(&selector).await?;
    let report = engine
        .sync(filter, &resolver, &resolver)
        .await
        .map_err(to_anyhow)?;
    notify_cache_of_pull(&backend, &resolver.index, &report.pulled).await;
    let total_failed = report.pulled.failed.len() + report.pushed.total_failed();
    tracing::info!(
        remotes = %label,
        updated = report.pulled.updated.len(),
        new_groups = report.pulled.new_groups.len(),
        pushed = report.pushed.total_pushed(),
        failed = total_failed,
        "sync completed"
    );
    println!(
        "sync against {} completed: pulled {} groups ({} new), pushed {} groups, {} groups failed",
        label,
        report.pulled.updated.len(),
        report.pulled.new_groups.len(),
        report.pushed.total_pushed(),
        total_failed
    );
    let mut notes = sync_push_partial_failure_notes(&report.pushed);
    notes.extend(sync_group_failure_notes(
        &report.pulled.failed,
        "pull",
        &label,
    ));
    for (remote_name, failures) in report
        .pushed
        .by_remote
        .iter()
        .map(|outcome| (outcome.remote_name.as_str(), outcome.failed.as_slice()))
    {
        notes.extend(sync_group_failure_notes(failures, "push", remote_name));
    }
    render_notes_tail(&notes);
    if total_failed > 0 {
        bail!(
            "sync against {} failed for {} groups (pull + push combined); see notes above for per-group errors",
            label,
            total_failed
        );
    }
    Ok(())
}

fn to_anyhow(err: SyncError) -> anyhow::Error {
    anyhow::Error::from(err)
}

/// Pull-trigger wiring for the local content cache: re-index
/// exactly the groups `report` says advanced. Best-effort via
/// [`mmcp_store::cache::notify_pull`]: never fails the `pull` /
/// `sync` command it observes.
async fn notify_cache_of_pull(
    backend: &mmcp_git::NativeBackend,
    groups: &mmcp_store::GroupIndex,
    report: &mmcp_sync::PullReport,
) {
    let updated: Vec<uuid::Uuid> = report.updated.iter().map(|g| g.group_id).collect();
    mmcp_store::cache::notify_pull(backend, groups, &updated).await;
}
