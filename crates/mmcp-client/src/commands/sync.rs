//! `mmcp sync`, `mmcp pull`, and `mmcp push` implementations.
//!
//! All three subcommands share this entry point: the booleans
//! `pull` and `push` toggle which halves of a full sync run. The
//! body calls through to `mmcp_store::sync::build_engine` (the
//! shared wiring used by the MCP tools as well) and formats the
//! resulting report for stdout.
//!
//! Every sync verb resolves against the EFFECTIVE remote set (user
//! config plus project config, merged by
//! `mmcp_store::resolve_effective_remotes`), not a single
//! `server_url`: a project may configure zero, one, or several
//! remotes at either level. `mmcp push` and `mmcp sync`'s push half
//! both default to `PushScope::Default` (the resolved default remote
//! only); `--all-remotes` / `--remote <name>` on either subcommand
//! (shared via [`RemoteScopeArgs`]) select `PushScope::All` /
//! `PushScope::Named`. `mmcp fetch` and `mmcp pull` carry neither
//! flag: they stay read-only and unaffected by push scoping.

use anyhow::{Context, Result, bail};
use mmcp_core::id::GroupId;
use mmcp_core::manifest::GroupScope;
use mmcp_store::config::{find_project_root, load};
use mmcp_store::home::MmcpHome;
use mmcp_store::memory::resolve_group;
use mmcp_store::sync::build_engine;
use mmcp_store::{EffectiveRemotes, GroupIndex, resolve_effective_remotes};
use mmcp_sync::{PushScope, SyncError, SyncFilter};

use crate::commands::sync_table::{SyncRowAction, SyncTableRow, render_sync_table};
use crate::notes::{
    render_notes_tail, sync_group_failure_notes, sync_manifest_failure_notes,
    sync_push_partial_failure_notes,
};

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

/// Selector shared by `mmcp sync` / `pull` / `push` / `fetch`.
///
/// At most one of `--group` / `--scope` / `--all` may be given.
/// clap's `ArgGroup(multiple = false)` enforces mutual exclusion at parse time.
/// Omitting all three targets every locally-known group, the same as passing `--all` explicitly.
#[derive(Debug, Clone, clap::Args)]
#[group(id = "sync_selector", multiple = false)]
pub struct SyncSelector {
    /// Target a single group by UUID or slug.
    #[arg(long, group = "sync_selector")]
    pub group: Option<String>,

    /// Target every locally-known group whose manifest carries the
    /// chosen `GroupScope`.
    #[arg(long, group = "sync_selector", value_enum)]
    pub scope: Option<ScopeArg>,

    /// Explicitly fan out across the whole local mirror.
    /// Redundant with the selector's own default when neither `--group` nor `--scope` is given.
    /// Still accepted so a caller can name the whole-mirror target explicitly.
    #[arg(long, group = "sync_selector", default_value_t = false)]
    pub all: bool,
}

/// Push remote-selection flags shared by `mmcp push` and `mmcp
/// sync`'s push half. Orthogonal to [`SyncSelector`], which picks
/// which GROUPS are touched: this picks which REMOTE(S) receive the
/// push, and the two families combine freely (e.g. `mmcp push
/// --scope shared --all-remotes`).
///
/// At most one of `--all-remotes` / `--remote` may be given, and
/// neither is required: [`RemoteScopeArgs::resolve`] maps the
/// no-flag case to `PushScope::Default`.
#[derive(Debug, Clone, clap::Args)]
#[group(id = "push_remote_scope", multiple = false)]
pub struct RemoteScopeArgs {
    /// Push to every effective remote whose `include_in_push_all` is
    /// not `false`, instead of the resolved default remote only.
    #[arg(long, group = "push_remote_scope")]
    pub all_remotes: bool,

    /// Push to exactly this remote by name, instead of the resolved
    /// default remote. Must match a remote in the effective set; an
    /// unknown name is a clear CLI error naming the requested name.
    #[arg(long, group = "push_remote_scope", value_name = "NAME")]
    pub remote: Option<String>,
}

impl RemoteScopeArgs {
    /// Resolve to the [`PushScope`] this CLI invocation selected.
    #[must_use]
    pub fn resolve(&self) -> PushScope {
        if self.all_remotes {
            PushScope::All
        } else if let Some(name) = &self.remote {
            PushScope::Named(name.clone())
        } else {
            PushScope::Default
        }
    }
}

/// Resolve a [`SyncSelector`] into a [`SyncFilter`] against the
/// live group index.
///
/// An empty selector (none of `--group`, `--scope`, `--all` given) resolves to [`SyncFilter::All`].
/// This matches the selector's own documented default.
pub async fn resolve_sync_filter(
    selector: &SyncSelector,
    groups: &GroupIndex,
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

/// Load the project + user config, resolve the effective remote set,
/// and bail loudly when it is empty: every sync verb needs at least
/// one remote to do anything.
async fn load_effective_remotes(root: &std::path::Path) -> Result<EffectiveRemotes> {
    let project_cfg = load(root)?;
    let mmcp_home = MmcpHome::discover()?;
    let user_cfg = mmcp_home.load_user_config()?;
    let effective = resolve_effective_remotes(&user_cfg, &project_cfg)?;
    if effective.is_empty() {
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
    let label = effective.summary_label();

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
        manifest_failures = report.manifest_failures.len(),
        "fetch completed"
    );
    println!(
        "fetch from {}: {} groups tracked, {} failed",
        label,
        report.groups.len(),
        report.failed.len() + report.manifest_failures.len()
    );
    let rows = fetch_table_rows(&report, &resolver.index).await;
    if let Some(table) = render_sync_table(&rows) {
        println!("{table}");
    }
    let mut notes = sync_group_failure_notes(&report.failed, "fetch", &label);
    notes.extend(sync_manifest_failure_notes(
        &report.manifest_failures,
        "fetch",
    ));
    render_notes_tail(&notes);
    // A3 partial-success fix: an earlier-in-order group's failure no
    // longer discards a later group's success (see
    // `mmcp_sync::engine::GroupSyncFailure`'s doc comment), but the
    // command must still exit non-zero when any group failed, or a
    // real failure would silently read as success. An unreachable
    // remote's manifest failure is the same kind of partial failure,
    // now surfaced instead of aborting the whole call: still a
    // failure the exit code must reflect.
    if !report.failed.is_empty() || !report.manifest_failures.is_empty() {
        bail!(
            "fetch from {} failed for {} of {} groups, {} remote manifest(s) unreachable; see \
             notes above for per-group and per-remote errors",
            label,
            report.failed.len(),
            report.failed.len() + report.groups.len(),
            report.manifest_failures.len()
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
    let label = effective.summary_label();

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
        manifest_failures = report.manifest_failures.len(),
        "pull completed"
    );
    println!(
        "pull from {}: {} groups updated, {} failed",
        label,
        report.updated.len(),
        report.failed.len() + report.manifest_failures.len()
    );
    let rows = pull_table_rows(&report, &resolver.index, &label).await;
    if let Some(table) = render_sync_table(&rows) {
        println!("{table}");
    }
    let mut notes = sync_group_failure_notes(&report.failed, "pull", &label);
    notes.extend(sync_manifest_failure_notes(
        &report.manifest_failures,
        "pull",
    ));
    render_notes_tail(&notes);
    if !report.failed.is_empty() || !report.manifest_failures.is_empty() {
        bail!(
            "pull from {} failed for {} of {} groups, {} remote manifest(s) unreachable; see \
             notes above for per-group and per-remote errors",
            label,
            report.failed.len(),
            report.failed.len() + report.updated.len(),
            report.manifest_failures.len()
        );
    }
    Ok(())
}

/// Run `mmcp push`: walk each in-scope group and ship local `main`
/// to the remote(s) `remote_scope` selects (the resolved default
/// remote when neither `--all-remotes` nor `--remote <name>` is
/// given).
pub async fn run_push(selector: SyncSelector, remote_scope: RemoteScopeArgs) -> Result<()> {
    let (label, engine, resolver, filter, _backend) = prepare(&selector).await?;
    let report = engine
        .push(filter, remote_scope.resolve(), &resolver, &resolver)
        .await
        .map_err(to_anyhow)?;
    tracing::info!(
        remotes = %label,
        pushed = report.total_pushed(),
        failed = report.total_failed(),
        "push completed"
    );
    println!(
        "push to {}: {} groups pushed, {} failed",
        label,
        report.total_pushed(),
        report.total_failed()
    );
    let rows = push_table_rows(&report, &resolver.index).await;
    if let Some(table) = render_sync_table(&rows) {
        println!("{table}");
    }
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
/// scoped per `remote_scope` (the resolved default remote when
/// neither `--all-remotes` nor `--remote <name>` is given).
///
/// Calls `engine.pull` then `engine.push` directly instead of the
/// engine's own `sync` convenience method, which fixes its push
/// phase to `PushScope::Default` with no override: replicating
/// `sync`'s pull-then-push behaviour and report assembly here is
/// what lets this CLI-only flag reach the push phase without
/// changing `SyncEngine::sync`'s signature (and its other callers,
/// the MCP `sync` tool and `mmcp-sync`'s own tests).
///
/// Exit-code contract (returned via `Result`): `Ok(())` on success,
/// an `anyhow::Error` carrying `SyncError::Conflict` on conflict
/// (caller maps to exit 2), any other `anyhow::Error` generic (1).
pub async fn run_sync(selector: SyncSelector, remote_scope: RemoteScopeArgs) -> Result<()> {
    let (label, engine, resolver, filter, backend) = prepare(&selector).await?;
    let pulled = engine
        .pull(filter, &resolver, &resolver)
        .await
        .map_err(to_anyhow)?;
    notify_cache_of_pull(&backend, &resolver.index, &pulled).await;
    let pushed = engine
        .push(filter, remote_scope.resolve(), &resolver, &resolver)
        .await
        .map_err(to_anyhow)?;
    let report = mmcp_sync::SyncReport { pulled, pushed };
    let total_failed = report.pulled.failed.len()
        + report.pulled.manifest_failures.len()
        + report.pushed.total_failed();
    tracing::info!(
        remotes = %label,
        updated = report.pulled.updated.len(),
        new_groups = report.pulled.new_groups.len(),
        pushed = report.pushed.total_pushed(),
        manifest_failures = report.pulled.manifest_failures.len(),
        failed = total_failed,
        "sync completed"
    );
    println!(
        "sync against {}: {} pulled, {} pushed, {} failed",
        label,
        report.pulled.updated.len(),
        report.pushed.total_pushed(),
        total_failed
    );
    let mut rows = pull_table_rows(&report.pulled, &resolver.index, &label).await;
    rows.extend(push_table_rows(&report.pushed, &resolver.index).await);
    if let Some(table) = render_sync_table(&rows) {
        println!("{table}");
    }
    let mut notes = sync_push_partial_failure_notes(&report.pushed);
    notes.extend(sync_group_failure_notes(
        &report.pulled.failed,
        "pull",
        &label,
    ));
    notes.extend(sync_manifest_failure_notes(
        &report.pulled.manifest_failures,
        "pull",
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

/// Table row group-column label.
/// The group's own indexed manifest slug wins; `advertised` is next; the raw UUID is the final fallback.
async fn group_label(index: &GroupIndex, group_id: uuid::Uuid, advertised: Option<&str>) -> String {
    if let Some(entry) = index.get(&GroupId::from_uuid(group_id)).await {
        return entry.manifest.slug;
    }
    advertised
        .map(str::to_string)
        .unwrap_or_else(|| group_id.to_string())
}

/// Build `mmcp fetch`'s per-group / per-remote outcome rows.
async fn fetch_table_rows(
    report: &mmcp_sync::FetchReport,
    index: &GroupIndex,
) -> Vec<SyncTableRow> {
    let mut rows = Vec::new();
    for g in &report.groups {
        let label = group_label(index, g.group_id, g.slug.as_deref()).await;
        let action = if g.ref_updated {
            SyncRowAction::Updated
        } else {
            SyncRowAction::Skipped
        };
        rows.push(SyncTableRow::new(label, action).with_remote(g.remote_name.clone()));
    }
    for g in &report.new_groups {
        rows.push(SyncTableRow::new(g.slug.clone(), SyncRowAction::New));
    }
    for f in &report.failed {
        let label = group_label(index, *f.group_id.as_uuid(), None).await;
        rows.push(SyncTableRow::new(label, SyncRowAction::Failed).with_detail(f.error.to_string()));
    }
    for m in &report.manifest_failures {
        rows.push(
            SyncTableRow::new("-", SyncRowAction::Unreachable)
                .with_remote(m.remote_name.clone())
                .with_detail(m.error.to_string()),
        );
    }
    rows
}

/// Build `mmcp pull`'s per-group outcome rows.
/// Every row belongs to the same implicit remote (`remote_label`).
/// See [`mmcp_sync::PullReport`]'s own doc comment for why the report carries no per-row remote.
async fn pull_table_rows(
    report: &mmcp_sync::PullReport,
    index: &GroupIndex,
    remote_label: &str,
) -> Vec<SyncTableRow> {
    let mut rows = Vec::new();
    for g in &report.updated {
        let label = group_label(index, g.group_id, Some(&g.slug)).await;
        rows.push(SyncTableRow::new(label, SyncRowAction::Updated).with_remote(remote_label));
    }
    for g in &report.new_groups {
        rows.push(SyncTableRow::new(g.slug.clone(), SyncRowAction::New).with_remote(remote_label));
    }
    for f in &report.failed {
        let label = group_label(index, *f.group_id.as_uuid(), None).await;
        rows.push(SyncTableRow::new(label, SyncRowAction::Failed).with_detail(f.error.to_string()));
    }
    for m in &report.manifest_failures {
        rows.push(
            SyncTableRow::new("-", SyncRowAction::Unreachable)
                .with_remote(m.remote_name.clone())
                .with_detail(m.error.to_string()),
        );
    }
    rows
}

/// Build `mmcp push`'s per-group, per-remote outcome rows.
async fn push_table_rows(report: &mmcp_sync::PushReport, index: &GroupIndex) -> Vec<SyncTableRow> {
    let mut rows = Vec::new();
    for outcome in &report.by_remote {
        for g in &outcome.pushed {
            let label = group_label(index, g.group_id, None).await;
            let action = if g.content_transferred {
                SyncRowAction::Pushed
            } else {
                SyncRowAction::Skipped
            };
            let mut row = SyncTableRow::new(label, action).with_remote(outcome.remote_name.clone());
            if let Some(err) = &g.transport_error {
                row = row.with_detail(err.to_string());
            }
            rows.push(row);
        }
        for f in &outcome.failed {
            let label = group_label(index, *f.group_id.as_uuid(), None).await;
            rows.push(
                SyncTableRow::new(label, SyncRowAction::Failed)
                    .with_remote(outcome.remote_name.clone())
                    .with_detail(f.error.to_string()),
            );
        }
    }
    rows
}

/// Pull-trigger wiring for the local content cache: re-index
/// exactly the groups `report` says advanced. Best-effort via
/// [`mmcp_store::cache::notify_pull`]: never fails the `pull` /
/// `sync` command it observes.
async fn notify_cache_of_pull(
    backend: &mmcp_git::NativeBackend,
    groups: &GroupIndex,
    report: &mmcp_sync::PullReport,
) {
    let updated: Vec<uuid::Uuid> = report.updated.iter().map(|g| g.group_id).collect();
    mmcp_store::cache::notify_pull(backend, groups, &updated).await;
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    fn empty_selector() -> SyncSelector {
        SyncSelector {
            group: None,
            scope: None,
            all: false,
        }
    }

    #[tokio::test]
    async fn resolve_sync_filter_defaults_the_empty_selector_to_all() {
        let scratch = mmcp_store::testing::ScratchHome::new()
            .await
            .expect("scratch home");
        let filter = resolve_sync_filter(&empty_selector(), scratch.groups())
            .await
            .expect("empty selector resolves");
        assert!(matches!(filter, SyncFilter::All));
    }

    #[tokio::test]
    async fn resolve_sync_filter_honors_explicit_all() {
        let scratch = mmcp_store::testing::ScratchHome::new()
            .await
            .expect("scratch home");
        let selector = SyncSelector {
            all: true,
            ..empty_selector()
        };
        let filter = resolve_sync_filter(&selector, scratch.groups())
            .await
            .expect("explicit --all resolves");
        assert!(matches!(filter, SyncFilter::All));
    }

    #[tokio::test]
    async fn resolve_sync_filter_honors_explicit_scope() {
        let scratch = mmcp_store::testing::ScratchHome::new()
            .await
            .expect("scratch home");
        let selector = SyncSelector {
            scope: Some(ScopeArg::Global),
            ..empty_selector()
        };
        let filter = resolve_sync_filter(&selector, scratch.groups())
            .await
            .expect("explicit --scope resolves");
        assert!(matches!(filter, SyncFilter::Scope(GroupScope::Global)));
    }

    #[tokio::test]
    async fn resolve_sync_filter_honors_explicit_group() {
        let scratch = mmcp_store::testing::ScratchHome::new()
            .await
            .expect("scratch home");
        let seeded = scratch
            .seed_group("resolve-filter-group")
            .await
            .expect("seed group");
        let selector = SyncSelector {
            group: Some(seeded.group_id.to_string()),
            ..empty_selector()
        };
        let filter = resolve_sync_filter(&selector, scratch.groups())
            .await
            .expect("explicit --group resolves");
        assert_eq!(filter, SyncFilter::Group(*seeded.group_id.as_uuid()));
    }
}
