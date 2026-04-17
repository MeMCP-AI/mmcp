//! CLI surface for the feature-request tools (FR-007).
//!
//! Thin adapters over `mmcp_store::features`: each subcommand
//! resolves the project group from `cwd`, calls the corresponding
//! store function, and prints a compact human-readable block. JSON
//! output is out of scope — the MCP tool surface is the canonical
//! machine-readable path, and scripted pipelines can call that
//! directly via the stdio server rather than scraping CLI output.

use anyhow::{Context, Result};
use clap::{Args, Subcommand};
use mmcp_core::memory::FeatureStatus;
use mmcp_store::features::{
    AddSpec, FeatureRecord, UpdateSpec, add_feature, delete_feature, list_features, read_feature,
    resolve_project_group, update_feature,
};
use mmcp_store::home::MmcpHome;

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
    /// needs work?". FR-024.
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
    }
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
    let spec = AddSpec {
        slug: args.slug,
        title: args.title.unwrap_or_default(),
        description: args.description,
        body: read_body(&args.body)?,
        status,
        depends_on: args.depends_on,
        blocks: args.blocks,
        message: args.message,
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
        Some(raw) => Some(read_body(&raw)?),
        None => None,
    };
    let depends_on = if args.depends_on_clear {
        Some(Vec::new())
    } else if args.depends_on.is_empty() {
        None
    } else {
        Some(args.depends_on)
    };
    let blocks = if args.blocks_clear {
        Some(Vec::new())
    } else if args.blocks.is_empty() {
        None
    } else {
        Some(args.blocks)
    };

    let spec = UpdateSpec {
        title: args.title,
        description: args.description,
        body,
        status,
        depends_on,
        blocks,
        message: args.message,
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
    let records = list_features(&backend, &entry, status_filter, args.all)
        .await
        .map_err(anyhow::Error::from)?;

    if records.is_empty() {
        match (status_filter, args.all) {
            (Some(s), _) => println!("no feature requests with status `{}`", s.as_str()),
            (None, true) => println!("no feature requests filed in this project yet"),
            (None, false) => println!(
                "no open feature requests in this project; pass --all to include closed ones"
            ),
        }
        return Ok(());
    }

    for record in &records {
        print_record_summary(record);
    }
    println!("\n{} feature(s)", records.len());
    Ok(())
}

// ── Output helpers ──────────────────────────────────────────────

fn print_record_summary(record: &FeatureRecord) {
    let title = if record.title.is_empty() {
        "(untitled)"
    } else {
        record.title.as_str()
    };
    println!("[{}] {} — {}", record.status.as_str(), record.slug, title);
    if !record.description.is_empty() {
        println!("    {}", record.description);
    }
}

fn print_record_full(record: &FeatureRecord) {
    println!("slug        : {}", record.slug);
    println!("title       : {}", record.title);
    println!("status      : {}", record.status.as_str());
    if !record.description.is_empty() {
        println!("description : {}", record.description);
    }
    if !record.depends_on.is_empty() {
        println!("depends_on  : {}", record.depends_on.join(", "));
    }
    if !record.blocks.is_empty() {
        println!("blocks      : {}", record.blocks.join(", "));
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
/// human-readable error when the value is not one of the five
/// variants so operators see the acceptable options immediately
/// without a crash trace.
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

/// Interpret a body argument: the literal `-` reads from stdin so
/// operators can pipe in markdown without quoting hell; any other
/// value is used verbatim.
fn read_body(raw: &str) -> Result<String> {
    use std::io::Read;
    if raw == "-" {
        let mut buf = String::new();
        std::io::stdin()
            .read_to_string(&mut buf)
            .context("reading feature body from stdin")?;
        Ok(buf)
    } else {
        Ok(raw.to_string())
    }
}
