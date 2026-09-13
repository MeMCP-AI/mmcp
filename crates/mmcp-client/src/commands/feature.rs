//! CLI surface for the feature-request tools.
//!
//! Thin adapters over `mmcp_store::features`: each subcommand
//! resolves the project group from `cwd`, calls the corresponding
//! store function, and prints a compact human-readable block. JSON
//! output is out of scope: the MCP tool surface is the canonical
//! machine-readable path, and scripted pipelines can call that
//! directly via the stdio server rather than scraping CLI output.

use anyhow::{Context, Result};
use clap::{Args, Subcommand};
use mmcp_core::memory::FeatureStatus;
use mmcp_proto::Note;
use mmcp_store::features::{
    AddSpec, FeatureRecord, FeatureSummary, UpdateSpec, add_feature, delete_feature,
    list_feature_summaries, read_feature, resolve_project_group, update_feature,
};
use mmcp_store::home::MmcpHome;

use crate::commands::tracker_cli::{join_uuids, read_body};
use crate::notes::{
    collect_known_memory_ids, dangling_ref_notes_for, dangling_ref_notes_with_known,
    findings_to_notes, render_notes_tail,
};

// ── Clap surface ────────────────────────────────────────────────

/// Top-level arg wrapper for `mmcp feature`. A missing subcommand
/// prints help rather than running silently.
#[derive(Debug, Args)]
#[command(arg_required_else_help = true)]
pub struct FeatureArgs {
    #[command(subcommand)]
    pub cmd: Option<FeatureCommand>,
}

#[derive(Debug, Subcommand)]
pub enum FeatureCommand {
    /// File a new feature request in the current project's group.
    Add(AddArgs),
    /// Read a feature request by slug.
    Read(ReadArgs),
    /// Apply partial updates to an existing feature request.
    Update(UpdateArgs),
    /// Delete a feature request by slug.
    Delete(DeleteArgs),
    /// List feature requests, optionally filtered by status.
    List(ListArgs),
    /// Rename every feature under a slug to a new slug.
    Rename(RenameArgs),
}

#[derive(Debug, Args, Default)]
pub struct AddArgs {
    /// Stable slug.
    /// Auto-minted from the title when omitted.
    /// Prefer an explicit slug grouped under a subject prefix (`<area>/<short-slug>`).
    #[arg(long)]
    pub slug: Option<String>,

    /// Human-readable title. Required unless `--slug` is provided.
    #[arg(long)]
    pub title: Option<String>,

    /// One-line summary.
    #[arg(long, default_value_t = String::new())]
    pub description: String,

    /// Full FR body as markdown. Pass `-` to read from stdin.
    #[arg(long, default_value_t = String::new())]
    pub body: String,

    /// Initial lifecycle state. Defaults to `open`.
    #[arg(long)]
    pub status: Option<String>,

    /// Slugs of prerequisites. Repeat the flag for each entry.
    #[arg(long = "depends-on")]
    pub depends_on: Vec<String>,

    /// Slugs this FR blocks. Repeat the flag for each entry.
    #[arg(long = "blocks")]
    pub blocks: Vec<String>,

    /// UUID of the milestone this feature counts toward. Cross-group
    /// by design (D3): the milestone does not have to live in this
    /// project's group.
    #[arg(long)]
    pub milestone: Option<String>,

    /// Override for the git commit message.
    #[arg(long)]
    pub message: Option<String>,
}

#[derive(Debug, Args)]
pub struct ReadArgs {
    /// Slug to read.
    pub slug: String,

    /// Branch, tag, or 40-char commit hex. Defaults to `main`.
    #[arg(long)]
    pub version: Option<String>,
}

#[derive(Debug, Args, Default)]
pub struct UpdateArgs {
    /// Slug of the FR to mutate.
    pub slug: String,

    /// New title. Omit to leave unchanged.
    #[arg(long)]
    pub title: Option<String>,

    /// New description. Omit to leave unchanged.
    #[arg(long)]
    pub description: Option<String>,

    /// New body. Omit to leave unchanged.
    #[arg(long)]
    pub body: Option<String>,

    /// New status. Omit to leave unchanged.
    #[arg(long)]
    pub status: Option<String>,

    /// Replacement `depends_on` list. Omit to leave unchanged; pass
    /// `--depends-on-clear` to empty the list.
    #[arg(long = "depends-on")]
    pub depends_on: Vec<String>,

    /// Clear the `depends_on` list. Mutually exclusive with
    /// `--depends-on`.
    #[arg(long, conflicts_with = "depends_on")]
    pub depends_on_clear: bool,

    /// Replacement `blocks` list. Omit to leave unchanged; pass
    /// `--blocks-clear` to empty the list.
    #[arg(long = "blocks")]
    pub blocks: Vec<String>,

    /// Clear the `blocks` list. Mutually exclusive with `--blocks`.
    #[arg(long, conflicts_with = "blocks")]
    pub blocks_clear: bool,

    /// Replacement milestone UUID. Omit to leave unchanged; pass
    /// `--milestone-clear` to unlink.
    #[arg(long, conflicts_with = "milestone_clear")]
    pub milestone: Option<String>,

    /// Clear the milestone link. Mutually exclusive with
    /// `--milestone`.
    #[arg(long)]
    pub milestone_clear: bool,

    /// Override for the git commit message.
    #[arg(long)]
    pub message: Option<String>,
}

#[derive(Debug, Args)]
pub struct RenameArgs {
    /// Current slug directory.
    pub old_slug: String,

    /// Target slug directory.
    /// Must satisfy the slug contract.
    /// Duplicate slugs are allowed, so this may land under an existing slug as a sibling.
    /// Group it under a subject prefix nested with `/`, never a hyphenated form.
    pub new_slug: String,

    /// Override for the git commit message.
    #[arg(long)]
    pub message: Option<String>,
}

#[derive(Debug, Args)]
pub struct DeleteArgs {
    /// Slug to delete.
    pub slug: String,

    /// Override for the git commit message.
    #[arg(long)]
    pub message: Option<String>,
}

#[derive(Debug, Args, Default)]
pub struct ListArgs {
    /// Restrict to FRs with this status. Explicit status wins
    /// over the default open-only hide, so `--status resolved`
    /// returns resolved FRs without needing `--all`.
    #[arg(long)]
    pub status: Option<String>,

    /// Include FRs whose status is not `open`. Without this flag
    /// the listing hides closed-like FRs (resolved, blocked,
    /// deferred, duplicate) so the default signal is "what still
    /// needs work?".
    #[arg(long)]
    pub all: bool,
}

// ── Dispatcher ──────────────────────────────────────────────────

pub async fn run(args: FeatureArgs) -> Result<()> {
    match args.cmd {
        // `arg_required_else_help` on `FeatureArgs` means clap
        // prints help before this branch; the arm exists to catch
        // future variants added without a dispatch update.
        None => unreachable!("clap enforces subcommand presence"),
        Some(FeatureCommand::Add(a)) => run_add(a).await,
        Some(FeatureCommand::Read(a)) => run_read(a).await,
        Some(FeatureCommand::Update(a)) => run_update(a).await,
        Some(FeatureCommand::Delete(a)) => run_delete(a).await,
        Some(FeatureCommand::List(a)) => run_list(a).await,
        Some(FeatureCommand::Rename(a)) => run_rename(a).await,
    }
}

async fn run_rename(args: RenameArgs) -> Result<()> {
    let cwd = std::env::current_dir().context("reading current working directory")?;
    let home = MmcpHome::discover()?;
    let (backend, groups) = home.init_backend().await?;
    let (entry, _root) = resolve_project_group(&groups, &cwd)
        .await
        .map_err(anyhow::Error::from)?;
    let author = home.resolve_author();

    let records = mmcp_store::rename_feature(
        &backend,
        &entry,
        &args.old_slug,
        &args.new_slug,
        &author,
        args.message.as_deref(),
    )
    .await
    .map_err(anyhow::Error::from)?;
    println!(
        "renamed {} feature(s) from `{}` to `{}`",
        records.len(),
        args.old_slug,
        args.new_slug
    );
    for record in &records {
        print_record_summary(record);
    }
    Ok(())
}

// ── Handlers ────────────────────────────────────────────────────

async fn run_add(args: AddArgs) -> Result<()> {
    let cwd = std::env::current_dir().context("reading current working directory")?;
    let home = MmcpHome::discover()?;
    let (backend, groups) = home.init_backend().await?;
    let (entry, _root) = resolve_project_group(&groups, &cwd)
        .await
        .map_err(anyhow::Error::from)?;
    let author = home.resolve_author();

    let status = parse_status_cli(args.status.as_deref())?.unwrap_or_default();
    let depends_on = mmcp_store::parse_cross_refs(&args.depends_on, "depends_on")
        .map_err(anyhow::Error::from)?;
    let blocks =
        mmcp_store::parse_cross_refs(&args.blocks, "blocks").map_err(anyhow::Error::from)?;
    let milestone = parse_milestone_cli(args.milestone.as_deref())?;
    let spec = AddSpec {
        slug: args.slug,
        title: args.title.unwrap_or_default(),
        description: args.description,
        body: read_body(&args.body, "feature")?,
        status,
        depends_on,
        blocks,
        milestone,
        message: args.message,
        // `refs`, `supersedes`, and `number` are not exposed on the
        // CLI. `number` is server-assigned only. Refs and
        // supersedes are MCP-only until the CLI UX is designed.
        ..AddSpec::default()
    };
    let record = add_feature(&backend, &entry, spec, &author)
        .await
        .map_err(anyhow::Error::from)?;

    println!(
        "created feature `{}` ({})\n  status: {}\n  commit: {}",
        record.slug,
        record.title,
        record.status.as_str(),
        record.commit_id
    );
    Ok(())
}

async fn run_read(args: ReadArgs) -> Result<()> {
    let cwd = std::env::current_dir().context("reading current working directory")?;
    let home = MmcpHome::discover()?;
    let (backend, groups) = home.init_backend().await?;
    let (entry, _root) = resolve_project_group(&groups, &cwd)
        .await
        .map_err(anyhow::Error::from)?;

    let record = read_feature(&backend, &entry, &args.slug, args.version.as_deref())
        .await
        .map_err(anyhow::Error::from)?;
    print_record_full(&record);
    let notes = dangling_ref_notes_for(
        &backend,
        &entry,
        &record.slug,
        &record.depends_on,
        &record.blocks,
        record.superseded_by.as_ref(),
    )
    .await;
    render_notes_tail(&notes);
    Ok(())
}

async fn run_update(args: UpdateArgs) -> Result<()> {
    let cwd = std::env::current_dir().context("reading current working directory")?;
    let home = MmcpHome::discover()?;
    let (backend, groups) = home.init_backend().await?;
    let (entry, _root) = resolve_project_group(&groups, &cwd)
        .await
        .map_err(anyhow::Error::from)?;
    let author = home.resolve_author();

    let status = parse_status_cli(args.status.as_deref())?;
    let body = match args.body {
        Some(raw) => Some(read_body(&raw, "feature")?),
        None => None,
    };
    let depends_on = if args.depends_on_clear {
        Some(Vec::new())
    } else if args.depends_on.is_empty() {
        None
    } else {
        Some(
            mmcp_store::parse_cross_refs(&args.depends_on, "depends_on")
                .map_err(anyhow::Error::from)?,
        )
    };
    let blocks = if args.blocks_clear {
        Some(Vec::new())
    } else if args.blocks.is_empty() {
        None
    } else {
        Some(mmcp_store::parse_cross_refs(&args.blocks, "blocks").map_err(anyhow::Error::from)?)
    };
    let milestone = if args.milestone_clear {
        Some(None)
    } else {
        match args.milestone.as_deref() {
            Some(raw) => Some(Some(parse_milestone_uuid(raw)?)),
            None => None,
        }
    };

    let spec = UpdateSpec {
        title: args.title,
        description: args.description,
        body,
        status,
        depends_on,
        blocks,
        milestone,
        message: args.message,
        // CLI does not yet expose the refs / supersede knobs;
        // the MCP tools are the primary surface for now.
        ..UpdateSpec::default()
    };
    let record = update_feature(&backend, &entry, &args.slug, spec, &author)
        .await
        .map_err(anyhow::Error::from)?;
    println!(
        "updated feature `{}`\n  status: {}\n  commit: {}",
        record.slug,
        record.status.as_str(),
        record.commit_id
    );
    Ok(())
}

async fn run_delete(args: DeleteArgs) -> Result<()> {
    let cwd = std::env::current_dir().context("reading current working directory")?;
    let home = MmcpHome::discover()?;
    let (backend, groups) = home.init_backend().await?;
    let (entry, _root) = resolve_project_group(&groups, &cwd)
        .await
        .map_err(anyhow::Error::from)?;
    let author = home.resolve_author();

    let commit_id = delete_feature(
        &backend,
        &entry,
        &args.slug,
        &author,
        args.message.as_deref(),
    )
    .await
    .map_err(anyhow::Error::from)?;
    println!("deleted feature `{}` (commit {})", args.slug, commit_id);
    Ok(())
}

async fn run_list(args: ListArgs) -> Result<()> {
    let cwd = std::env::current_dir().context("reading current working directory")?;
    let home = MmcpHome::discover()?;
    let (backend, groups) = home.init_backend().await?;
    let (entry, _root) = resolve_project_group(&groups, &cwd)
        .await
        .map_err(anyhow::Error::from)?;

    let status_filter = parse_status_cli(args.status.as_deref())?;
    let (summaries, findings) = list_feature_summaries(&backend, &entry, status_filter, args.all)
        .await
        .map_err(anyhow::Error::from)?;

    if summaries.is_empty() {
        match (status_filter, args.all) {
            (Some(s), _) => println!("no feature requests with status `{}`", s.as_str()),
            (None, true) => println!("no feature requests filed in this project yet"),
            (None, false) => println!(
                "no open feature requests in this project; pass --all to include closed ones"
            ),
        }
        render_notes_tail(&findings_to_notes(&findings));
        return Ok(());
    }

    for summary in &summaries {
        print_summary_line(summary);
    }
    println!("\n{} feature(s)", summaries.len());

    let mut notes: Vec<Note> = findings_to_notes(&findings);
    let known = collect_known_memory_ids(&backend, &entry).await.ok();
    for summary in &summaries {
        if let Some(known) = &known {
            notes.extend(dangling_ref_notes_with_known(
                known,
                &entry,
                &summary.slug,
                &summary.depends_on,
                &summary.blocks,
                summary.superseded_by.as_ref(),
            ));
        }
    }
    render_notes_tail(&notes);
    Ok(())
}

// ── Output helpers ──────────────────────────────────────────────

fn print_listing_row(
    slug: &str,
    title: &str,
    description: &str,
    status: FeatureStatus,
    number: Option<u32>,
) {
    let title = if title.is_empty() {
        "(untitled)"
    } else {
        title
    };
    let number = number.map(|n| format!("#{n} ")).unwrap_or_default();
    println!("[{}] {}{} — {}", status.as_str(), number, slug, title);
    if !description.is_empty() {
        println!("    {description}");
    }
}

fn print_record_summary(record: &FeatureRecord) {
    print_listing_row(
        &record.slug,
        &record.title,
        &record.description,
        record.status,
        record.number,
    );
}

fn print_summary_line(summary: &FeatureSummary) {
    print_listing_row(
        &summary.slug,
        &summary.title,
        &summary.description,
        summary.status,
        summary.number,
    );
}

fn print_record_full(record: &FeatureRecord) {
    println!("slug        : {}", record.slug);
    if let Some(n) = record.number {
        println!("number      : {n}");
    }
    println!("title       : {}", record.title);
    println!("status      : {}", record.status.as_str());
    if !record.description.is_empty() {
        println!("description : {}", record.description);
    }
    if !record.depends_on.is_empty() {
        println!("depends_on  : {}", join_uuids(&record.depends_on));
    }
    if !record.blocks.is_empty() {
        println!("blocks      : {}", join_uuids(&record.blocks));
    }
    if let Some(milestone) = record.milestone {
        println!("milestone   : {milestone}");
    }
    if !record.commit_id.is_empty() {
        println!("commit      : {}", record.commit_id);
    }
    if !record.body.is_empty() {
        println!("\n{}", record.body);
    }
}

// ── Parsing helpers ─────────────────────────────────────────────

/// Parse the CLI wire form of [`FeatureStatus`]. Bails with a
/// human-readable error listing every valid variant, sourced from
/// [`FeatureStatus::all`], so operators see the acceptable options
/// immediately without a crash trace.
fn parse_status_cli(raw: Option<&str>) -> Result<Option<FeatureStatus>> {
    match raw {
        None => Ok(None),
        Some(s) => FeatureStatus::parse(s).map(Some).map_err(|err| {
            anyhow::anyhow!(
                "{err}\nhint: allowed values are {}",
                FeatureStatus::all()
                    .iter()
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join(" / ")
            )
        }),
    }
}

/// Parse an optional milestone UUID argument for `add`, where
/// absence simply means "no milestone".
fn parse_milestone_cli(raw: Option<&str>) -> Result<Option<uuid::Uuid>> {
    raw.map(parse_milestone_uuid).transpose()
}

/// Parse a milestone UUID argument, surfacing a clear error rather
/// than a bare `uuid::Error` when the operator passes a slug by
/// mistake: milestone cross-references are UUIDs only, never slugs.
fn parse_milestone_uuid(raw: &str) -> Result<uuid::Uuid> {
    uuid::Uuid::parse_str(raw)
        .map_err(|_| anyhow::anyhow!("`--milestone` expects a UUID, got '{raw}'"))
}
