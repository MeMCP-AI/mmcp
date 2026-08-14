//! CLI surface for `mmcp debug`: raw git access into a group's
//! bare repository. Mirrors the `mcp:debug_*` MCP tools, minus
//! `debug_toggle` (the toggle gates a long-running MCP session;
//! one-shot CLI invocations are gated by the operator typing
//! `mmcp debug` directly).
//!
//! Use cases: low-level inspection or repair when the typed CRUD
//! surface is not enough. Reach for `mmcp memory` first; this
//! namespace exists for the rare case where you need byte-level
//! access (corrupted manifest, history archaeology, etc.).

use anyhow::{Context, Result};
use clap::{Args, Subcommand};
use mmcp_git::{CommitSpec, GitBackend, Rev};
use mmcp_store::home::MmcpHome;
use mmcp_store::memory::resolve_group;

use crate::commands::import::protected_confirm;

#[derive(Debug, Args)]
#[command(arg_required_else_help = true)]
pub struct DebugArgs {
    #[command(subcommand)]
    pub cmd: Option<DebugCommand>,
}

#[derive(Debug, Subcommand)]
pub enum DebugCommand {
    /// Walk commit history for the whole repo or a single path.
    GitLog(GitLogArgs),
    /// List blobs under a path prefix at a revision.
    ListTree(ListTreeArgs),
    /// Read raw bytes for a file at a revision.
    ReadFile(ReadFileArgs),
    /// Write raw bytes to a file in the repo (commits the result).
    WriteFile(WriteFileArgs),
    /// Force a full rebuild of the local content cache, regardless
    /// of whether it already looks built. Reach for this after
    /// suspecting the cache has drifted from the git mirrors (e.g.
    /// a manual edit under `~/.mmcp/repos` outside the normal write
    /// path) rather than deleting the cache database by hand.
    CacheRebuild,
}

#[derive(Debug, Args)]
pub struct GitLogArgs {
    /// Group UUID or slug.
    pub group: String,

    /// Filter to a single path. Defaults to `.mmcp.toml` to
    /// mirror the MCP tool; the gix backend's `walk_history`
    /// requires a real path to resolve, so empty / unset returns
    /// nothing useful. Slice 4.7 will change both surfaces to a
    /// whole-repo log once the backend grows the primitive.
    #[arg(long)]
    pub path: Option<String>,

    /// Maximum number of commits to print. Default 20.
    #[arg(long, default_value_t = 20)]
    pub limit: usize,
}

#[derive(Debug, Args)]
pub struct ListTreeArgs {
    /// Group UUID or slug.
    pub group: String,

    /// Path prefix to walk. Default: repo root.
    #[arg(long)]
    pub prefix: Option<String>,

    /// Optional branch / tag / 40-char commit hex. Default HEAD.
    #[arg(long)]
    pub rev: Option<String>,
}

#[derive(Debug, Args)]
pub struct ReadFileArgs {
    /// Group UUID or slug.
    pub group: String,

    /// Path inside the repo (e.g. `memories/my-mem/<uuid>.md`,
    /// `.mmcp.toml`).
    pub path: String,

    /// Optional revision (branch / tag / commit hex). Default
    /// HEAD.
    #[arg(long)]
    pub rev: Option<String>,
}

#[derive(Debug, Args)]
pub struct WriteFileArgs {
    /// Group UUID or slug.
    pub group: String,

    /// Path inside the repo.
    pub path: String,

    /// File content. Literal string, `@PATH` to read from a
    /// file, or `-` for stdin.
    #[arg(long)]
    pub content: String,

    /// Override the git commit message.
    #[arg(long)]
    pub message: Option<String>,

    /// Pre-confirm a write into a protected group. The
    /// debug surface still gates protected groups so a raw write
    /// can't silently mutate shared rules.
    #[arg(long = "confirm-protected")]
    pub confirm_protected: bool,
}

pub async fn run(args: DebugArgs) -> Result<()> {
    match args.cmd {
        None => unreachable!("clap enforces subcommand presence"),
        Some(DebugCommand::GitLog(a)) => run_git_log(a).await,
        Some(DebugCommand::ListTree(a)) => run_list_tree(a).await,
        Some(DebugCommand::ReadFile(a)) => run_read_file(a).await,
        Some(DebugCommand::WriteFile(a)) => run_write_file(a).await,
        Some(DebugCommand::CacheRebuild) => run_cache_rebuild().await,
    }
}

/// Force a full rebuild of the local content cache across every
/// locally-mirrored group, regardless of whether the cache already
/// looks built. Unlike the lazy-build-on-read path (which only ever
/// runs once, the first time a query finds the index missing), this
/// always re-walks every group.
async fn run_cache_rebuild() -> Result<()> {
    let home = MmcpHome::discover()?;
    let (backend, groups) = home.init_backend().await?;
    let pool = mmcp_store::cache::open_pool(&mmcp_store::cache::default_db_path(&home))
        .await
        .context("opening local content cache database")?;
    let stats = mmcp_store::cache::rebuild_full(&pool, &backend, &groups)
        .await
        .context("rebuilding local content cache")?;
    println!(
        "cache rebuild complete: {} group(s) scanned, {} memory/memories indexed",
        stats.groups_scanned, stats.memories_indexed
    );
    Ok(())
}

async fn run_git_log(args: GitLogArgs) -> Result<()> {
    let home = MmcpHome::discover()?;
    let (backend, groups) = home.init_backend().await?;
    let entry = resolve_group(&groups, &args.group)
        .await
        .map_err(anyhow::Error::from)?;

    let path = args
        .path
        .as_deref()
        .unwrap_or(mmcp_core::manifest::MANIFEST_FILENAME);
    let history = backend
        .walk_history(&entry.handle, path)
        .await
        .map_err(anyhow::Error::from)?;
    if history.is_empty() {
        println!("(no commits)");
        return Ok(());
    }
    for commit in history.iter().take(args.limit) {
        let short: String = commit.id.chars().take(7).collect();
        println!("{}  {}  {}", short, commit.author_name, commit.subject);
    }
    let shown = history.len().min(args.limit);
    println!("\n{shown}/{} commit(s) shown", history.len());
    Ok(())
}

async fn run_list_tree(args: ListTreeArgs) -> Result<()> {
    let home = MmcpHome::discover()?;
    let (backend, groups) = home.init_backend().await?;
    let entry = resolve_group(&groups, &args.group)
        .await
        .map_err(anyhow::Error::from)?;
    let rev = parse_rev(args.rev.as_deref());
    let prefix = args.prefix.as_deref().unwrap_or("");
    let files = backend
        .list_tree(&entry.handle, prefix, &rev)
        .await
        .map_err(anyhow::Error::from)?;
    if files.is_empty() {
        println!("(no blobs under `{prefix}`)");
        return Ok(());
    }
    for file in &files {
        println!("{file}");
    }
    println!("\n{} file(s)", files.len());
    Ok(())
}

async fn run_read_file(args: ReadFileArgs) -> Result<()> {
    let home = MmcpHome::discover()?;
    let (backend, groups) = home.init_backend().await?;
    let entry = resolve_group(&groups, &args.group)
        .await
        .map_err(anyhow::Error::from)?;
    let rev = parse_rev(args.rev.as_deref());
    let bytes = backend
        .read_file(&entry.handle, &args.path, &rev)
        .await
        .map_err(anyhow::Error::from)?;
    let text = String::from_utf8_lossy(&bytes);
    print!("{text}");
    Ok(())
}

async fn run_write_file(args: WriteFileArgs) -> Result<()> {
    let content = read_content_input(&args.content)?;

    let home = MmcpHome::discover()?;
    let (backend, groups) = home.init_backend().await?;
    let entry = resolve_group(&groups, &args.group)
        .await
        .map_err(anyhow::Error::from)?;

    protected_confirm(&entry, args.confirm_protected)?;

    let author = home.resolve_author();
    let commit_id = backend
        .write_commit(
            &entry.handle,
            CommitSpec::mmcp_commit(
                args.message.as_deref().unwrap_or("debug: write file"),
                vec![(args.path.clone(), Some(content.into_bytes()))],
                &author.name,
                &author.email,
            ),
        )
        .await
        .map_err(anyhow::Error::from)?;
    println!("wrote `{}`\n  commit: {}", args.path, commit_id);
    Ok(())
}

// ── Helpers ─────────────────────────────────────────────────────

fn parse_rev(value: Option<&str>) -> Rev {
    match value {
        None => Rev::head(),
        Some(v) => {
            if v.len() == 40 && v.chars().all(|c| c.is_ascii_hexdigit()) {
                Rev::Commit(v.to_string())
            } else {
                Rev::Branch(v.to_string())
            }
        }
    }
}

fn read_content_input(raw: &str) -> Result<String> {
    use std::io::Read;
    if raw == "-" {
        let mut buf = String::new();
        std::io::stdin()
            .read_to_string(&mut buf)
            .context("reading content from stdin")?;
        Ok(buf)
    } else if let Some(path) = raw.strip_prefix('@') {
        std::fs::read_to_string(path).with_context(|| format!("reading content from {path}"))
    } else {
        Ok(raw.to_string())
    }
}
