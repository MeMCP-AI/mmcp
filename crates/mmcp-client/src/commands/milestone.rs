//! CLI surface for the milestone tracker.
//!
//! Sister to `commands::feature` / `commands::issue`, but a
//! deliberately reduced surface (M5 design): `add`, `read`,
//! `update`, `list` only — no `delete`, no `rename`. Thin adapters
//! over `mmcp_store::milestones`: each subcommand resolves the
//! project group from `cwd`, calls the corresponding store
//! function, and prints a compact human-readable block including
//! the freshly-computed cross-group rollup.

use anyhow::{Context, Result};
use clap::{Args, Subcommand};
use mmcp_core::memory::MilestoneStatus;
use mmcp_store::features::resolve_project_group;
use mmcp_store::home::MmcpHome;
use mmcp_store::milestones::{
    AddSpec, MilestoneRecord, UpdateSpec, add_milestone, list_milestones, read_milestone,
    update_milestone,
};
use mmcp_store::rollup::RollupStatus;

use crate::notes::{findings_to_notes, render_notes_tail};

// ── Clap surface ────────────────────────────────────────────────

/// Top-level arg wrapper for `mmcp milestone`. A missing subcommand
/// prints help rather than running silently.
#[derive(Debug, Args)]
#[command(arg_required_else_help = true)]
pub struct MilestoneArgs {
    #[command(subcommand)]
    pub cmd: Option<MilestoneCommand>,
}

#[derive(Debug, Subcommand)]
pub enum MilestoneCommand {
    /// File a new milestone in the current project's group.
    Add(AddArgs),
    /// Read a milestone by slug, with its live rollup.
    Read(ReadArgs),
    /// Apply partial updates to an existing milestone.
    Update(UpdateArgs),
    /// List milestones, with each one's live rollup.
    List(ListArgs),
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

    /// Full milestone body as markdown. Pass `-` to read from stdin.
    #[arg(long, default_value_t = String::new())]
    pub body: String,

    /// Initial editorial status. Defaults to `planning`. Allowed
    /// values: planning / active / on_hold / completed.
    #[arg(long)]
    pub status: Option<String>,

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
    /// Slug of the milestone to mutate.
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

    /// New editorial status. Omit to leave unchanged.
    #[arg(long)]
    pub status: Option<String>,

    /// Override for the git commit message.
    #[arg(long)]
    pub message: Option<String>,
}

#[derive(Debug, Args, Default)]
pub struct ListArgs {
    /// Include milestones whose live rollup is `completed`. Default
    /// listing hides these.
    #[arg(long)]
    pub all: bool,
}

// ── Dispatcher ──────────────────────────────────────────────────

pub async fn run(args: MilestoneArgs) -> Result<()> {
    match args.cmd {
        None => unreachable!("clap enforces subcommand presence"),
        Some(MilestoneCommand::Add(a)) => run_add(a).await,
        Some(MilestoneCommand::Read(a)) => run_read(a).await,
        Some(MilestoneCommand::Update(a)) => run_update(a).await,
        Some(MilestoneCommand::List(a)) => run_list(a).await,
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
    let spec = AddSpec {
        slug: args.slug,
        title: args.title.unwrap_or_default(),
        description: args.description,
        body: read_body(&args.body)?,
        status,
        message: args.message,
    };
    let record = add_milestone(&backend, &entry, spec, &author)
        .await
        .map_err(anyhow::Error::from)?;
    println!(
        "created milestone `{}` ({})\n  status: {}\n  commit: {}",
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
    mmcp_store::cache::init_from_home(&home)
        .await
        .map_err(|e| anyhow::anyhow!("failed to initialise local content cache: {e}"))?;
    let pool = mmcp_store::cache::active_pool()
        .ok_or_else(|| anyhow::anyhow!("local content cache is not available"))?;

    let record = read_milestone(
        &backend,
        &entry,
        &pool,
        &groups,
        &args.slug,
        args.version.as_deref(),
    )
    .await
    .map_err(anyhow::Error::from)?;
    print_record_full(&record);
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
    mmcp_store::cache::init_from_home(&home)
        .await
        .map_err(|e| anyhow::anyhow!("failed to initialise local content cache: {e}"))?;
    let pool = mmcp_store::cache::active_pool()
        .ok_or_else(|| anyhow::anyhow!("local content cache is not available"))?;

    let status = parse_status_cli(args.status.as_deref())?;
    let body = match args.body {
        Some(raw) => Some(read_body(&raw)?),
        None => None,
    };
    let spec = UpdateSpec {
        title: args.title,
        description: args.description,
        body,
        status,
        message: args.message,
    };
    let record = update_milestone(&backend, &entry, &pool, &groups, &args.slug, spec, &author)
        .await
        .map_err(anyhow::Error::from)?;
    println!(
        "updated milestone `{}`\n  status: {}\n  commit: {}",
        record.slug,
        record.status.as_str(),
        record.commit_id
    );
    Ok(())
}

async fn run_list(args: ListArgs) -> Result<()> {
    let cwd = std::env::current_dir().context("reading current working directory")?;
    let home = MmcpHome::discover()?;
    let (backend, groups) = home.init_backend().await?;
    let (entry, _root) = resolve_project_group(&groups, &cwd)
        .await
        .map_err(anyhow::Error::from)?;
    mmcp_store::cache::init_from_home(&home)
        .await
        .map_err(|e| anyhow::anyhow!("failed to initialise local content cache: {e}"))?;
    let pool = mmcp_store::cache::active_pool()
        .ok_or_else(|| anyhow::anyhow!("local content cache is not available"))?;

    let (records, findings) = list_milestones(&backend, &entry, &pool, &groups, args.all)
        .await
        .map_err(anyhow::Error::from)?;

    if records.is_empty() {
        if args.all {
            println!("no milestones filed in this project yet");
        } else {
            println!(
                "no in-progress milestones in this project; pass --all to include completed ones"
            );
        }
        render_notes_tail(&findings_to_notes(&findings));
        return Ok(());
    }

    for record in &records {
        print_listing_row(record);
    }
    println!("\n{} milestone(s)", records.len());
    render_notes_tail(&findings_to_notes(&findings));
    Ok(())
}

// ── Output helpers ──────────────────────────────────────────────

fn print_listing_row(record: &MilestoneRecord) {
    let title = if record.title.is_empty() {
        "(untitled)"
    } else {
        record.title.as_str()
    };
    println!(
        "[{} / rollup:{}] {} — {} ({}/{} completed{})",
        record.status.as_str(),
        record.rollup.status.as_str(),
        record.slug,
        title,
        record.rollup.completed,
        record.rollup.counted,
        if record.rollup.blocked > 0 {
            format!(", {} blocked", record.rollup.blocked)
        } else {
            String::new()
        }
    );
    if !record.description.is_empty() {
        println!("    {}", record.description);
    }
}

fn print_record_full(record: &MilestoneRecord) {
    println!("slug        : {}", record.slug);
    println!("title       : {}", record.title);
    println!("status      : {}", record.status.as_str());
    println!(
        "rollup      : {} ({}/{} completed{})",
        record.rollup.status.as_str(),
        record.rollup.completed,
        record.rollup.counted,
        if record.rollup.blocked > 0 {
            format!(", {} blocked", record.rollup.blocked)
        } else {
            String::new()
        }
    );
    if record.status.as_str() == "completed" && record.rollup.status != RollupStatus::Completed {
        println!(
            "            ! stale: editorial status is completed but the live rollup is not"
        );
    }
    if !record.description.is_empty() {
        println!("description : {}", record.description);
    }
    if !record.commit_id.is_empty() {
        println!("commit      : {}", record.commit_id);
    }
    if !record.body.is_empty() {
        println!("\n{}", record.body);
    }
}

// ── Parsing helpers ─────────────────────────────────────────────

fn parse_status_cli(raw: Option<&str>) -> Result<Option<MilestoneStatus>> {
    match raw {
        None => Ok(None),
        Some(s) => MilestoneStatus::parse(s).map(Some).map_err(|err| {
            anyhow::anyhow!(
                "{err}\nhint: allowed values are {}",
                MilestoneStatus::all()
                    .iter()
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join(" / ")
            )
        }),
    }
}

fn read_body(raw: &str) -> Result<String> {
    use std::io::Read;
    if raw == "-" {
        let mut buf = String::new();
        std::io::stdin()
            .read_to_string(&mut buf)
            .context("reading milestone body from stdin")?;
        Ok(buf)
    } else {
        Ok(raw.to_string())
    }
}
