//! CLI surface for the issue tracker tools.
//!
//! Sister to `commands::feature`. Thin adapters over
//! `mmcp_store::issues`: each subcommand resolves the project
//! group from `cwd`, calls the corresponding store function, and
//! prints a compact human-readable block. JSON output is out of
//! scope; the MCP tool surface is the canonical machine-readable
//! path.

use anyhow::{Context, Result};
use clap::{Args, Subcommand};
use mmcp_core::memory::IssueStatus;
use mmcp_proto::Note;
use mmcp_store::features::resolve_project_group;
use mmcp_store::home::MmcpHome;
use mmcp_store::issues::{
    AddSpec, IssueRecord, IssueSummary, UpdateSpec, add_issue, delete_issue, list_issue_summaries,
    read_issue, rename_issue, update_issue,
};

use crate::commands::tracker_cli::{join_uuids, read_body};
use crate::notes::{
    collect_known_memory_ids, dangling_ref_notes_for, dangling_ref_notes_with_known,
    findings_to_notes, render_notes_tail,
};

// ── Clap surface ────────────────────────────────────────────────

/// Top-level arg wrapper for `mmcp issue`. A missing subcommand
/// prints help rather than running silently.
#[derive(Debug, Args)]
#[command(arg_required_else_help = true)]
pub struct IssueArgs {
    #[command(subcommand)]
    pub cmd: Option<IssueCommand>,
}

#[derive(Debug, Subcommand)]
pub enum IssueCommand {
    /// File a new issue in the current project's group.
    Add(AddArgs),
    /// Read an issue by slug.
    Read(ReadArgs),
    /// Apply partial updates to an existing issue.
    Update(UpdateArgs),
    /// Delete an issue by slug.
    Delete(DeleteArgs),
    /// List issues, optionally filtered by status.
    List(ListArgs),
    /// Rename every issue under a slug to a new slug.
    Rename(RenameArgs),
}

#[derive(Debug, Args, Default)]
pub struct AddArgs {
    /// Stable slug. Auto-minted from the title when omitted.
    #[arg(long)]
    pub slug: Option<String>,

    /// Human-readable title. Required unless `--slug` is provided.
    #[arg(long)]
    pub title: Option<String>,

    /// One-line summary.
    #[arg(long, default_value_t = String::new())]
    pub description: String,

    /// Full issue body as markdown. Pass `-` to read from stdin.
    #[arg(long, default_value_t = String::new())]
    pub body: String,

    /// Initial lifecycle state. Defaults to `open`. Allowed values:
    /// open / closed / wontfix / blocked / deferred / duplicate /
    /// superseded.
    #[arg(long)]
    pub status: Option<String>,

    /// UUIDs of memories this issue depends on. Repeat for each.
    #[arg(long = "depends-on")]
    pub depends_on: Vec<String>,

    /// UUIDs of memories this issue blocks. Repeat for each.
    #[arg(long = "blocks")]
    pub blocks: Vec<String>,

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
    /// Slug of the issue to mutate.
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

    /// Replacement `depends_on` list. Omit to leave unchanged;
    /// pass `--depends-on-clear` to empty it.
    #[arg(long = "depends-on")]
    pub depends_on: Vec<String>,

    /// Clear the `depends_on` list.
    #[arg(long, conflicts_with = "depends_on")]
    pub depends_on_clear: bool,

    /// Replacement `blocks` list. Omit to leave unchanged.
    #[arg(long = "blocks")]
    pub blocks: Vec<String>,

    /// Clear the `blocks` list.
    #[arg(long, conflicts_with = "blocks")]
    pub blocks_clear: bool,

    /// Override for the git commit message.
    #[arg(long)]
    pub message: Option<String>,
}

#[derive(Debug, Args)]
pub struct RenameArgs {
    /// Current slug directory.
    pub old_slug: String,

    /// Target slug directory.
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
    /// Restrict to issues with this status.
    #[arg(long)]
    pub status: Option<String>,

    /// Include issues whose status is closed / wontfix /
    /// duplicate / superseded. Default listing hides these.
    #[arg(long)]
    pub all: bool,
}

// ── Dispatcher ──────────────────────────────────────────────────

pub async fn run(args: IssueArgs) -> Result<()> {
    match args.cmd {
        None => unreachable!("clap enforces subcommand presence"),
        Some(IssueCommand::Add(a)) => run_add(a).await,
        Some(IssueCommand::Read(a)) => run_read(a).await,
        Some(IssueCommand::Update(a)) => run_update(a).await,
        Some(IssueCommand::Delete(a)) => run_delete(a).await,
        Some(IssueCommand::List(a)) => run_list(a).await,
        Some(IssueCommand::Rename(a)) => run_rename(a).await,
    }
}

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
    let spec = AddSpec {
        slug: args.slug,
        title: args.title.unwrap_or_default(),
        description: args.description,
        body: read_body(&args.body, "issue")?,
        status,
        depends_on,
        blocks,
        message: args.message,
        ..AddSpec::default()
    };
    let record = add_issue(&backend, &entry, spec, &author)
        .await
        .map_err(anyhow::Error::from)?;
    println!(
        "created issue `{}` ({})\n  status: {}\n  commit: {}",
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

    let record = read_issue(&backend, &entry, &args.slug, args.version.as_deref())
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
        Some(raw) => Some(read_body(&raw, "issue")?),
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

    let spec = UpdateSpec {
        title: args.title,
        description: args.description,
        body,
        status,
        depends_on,
        blocks,
        message: args.message,
        ..UpdateSpec::default()
    };
    let record = update_issue(&backend, &entry, &args.slug, spec, &author)
        .await
        .map_err(anyhow::Error::from)?;
    println!(
        "updated issue `{}`\n  status: {}\n  commit: {}",
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

    let commit_id = delete_issue(
        &backend,
        &entry,
        &args.slug,
        &author,
        args.message.as_deref(),
    )
    .await
    .map_err(anyhow::Error::from)?;
    println!("deleted issue `{}` (commit {})", args.slug, commit_id);
    Ok(())
}

async fn run_rename(args: RenameArgs) -> Result<()> {
    let cwd = std::env::current_dir().context("reading current working directory")?;
    let home = MmcpHome::discover()?;
    let (backend, groups) = home.init_backend().await?;
    let (entry, _root) = resolve_project_group(&groups, &cwd)
        .await
        .map_err(anyhow::Error::from)?;
    let author = home.resolve_author();

    let records = rename_issue(
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
        "renamed {} issue(s) from `{}` to `{}`",
        records.len(),
        args.old_slug,
        args.new_slug
    );
    for record in &records {
        print_record_summary(record);
    }
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
    let (summaries, findings) = list_issue_summaries(&backend, &entry, status_filter, args.all)
        .await
        .map_err(anyhow::Error::from)?;

    if summaries.is_empty() {
        match (status_filter, args.all) {
            (Some(s), _) => println!("no issues with status `{}`", s.as_str()),
            (None, true) => println!("no issues filed in this project yet"),
            (None, false) => println!(
                "no open issues in this project; pass --all to include closed / wontfix / duplicate / superseded"
            ),
        }
        render_notes_tail(&findings_to_notes(&findings));
        return Ok(());
    }

    for summary in &summaries {
        print_summary_line(summary);
    }
    println!("\n{} issue(s)", summaries.len());

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
    status: IssueStatus,
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

fn print_record_summary(record: &IssueRecord) {
    print_listing_row(
        &record.slug,
        &record.title,
        &record.description,
        record.status,
        record.number,
    );
}

fn print_summary_line(summary: &IssueSummary) {
    print_listing_row(
        &summary.slug,
        &summary.title,
        &summary.description,
        summary.status,
        summary.number,
    );
}

fn print_record_full(record: &IssueRecord) {
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
    if !record.commit_id.is_empty() {
        println!("commit      : {}", record.commit_id);
    }
    if !record.body.is_empty() {
        println!("\n{}", record.body);
    }
}

// ── Parsing helpers ─────────────────────────────────────────────

fn parse_status_cli(raw: Option<&str>) -> Result<Option<IssueStatus>> {
    match raw {
        None => Ok(None),
        Some(s) => IssueStatus::parse(s).map(Some).map_err(|err| {
            anyhow::anyhow!(
                "{err}\nhint: allowed values are {}",
                IssueStatus::all()
                    .iter()
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join(" / ")
            )
        }),
    }
}
