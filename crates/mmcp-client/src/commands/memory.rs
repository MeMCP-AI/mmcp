//! CLI surface for memory CRUD (Slice 2a — read-only ops).
//!
//! Mirrors the read-only memory MCP tools: `list_memories`,
//! `read_memory`, `list_versions`, `read_memory_body_sections`,
//! and `search_memories`. Mutating ops (`write_memory`,
//! `edit_memory`, `edit_memory_body`, `delete_memory`) ship in
//! the next slice.
//!
//! Each subcommand resolves its group + memory through the
//! shared `mmcp_store::resolve_group` / `resolve_memory`
//! primitives so behaviour matches the MCP layer exactly. The
//! FR-45 notes channel rides on the read path via
//! `malformed_frontmatter_notes`; the FR-026 section reader uses
//! `mmcp_core::memory::body::parse_sections` directly.

use anyhow::{Context, Result};
use clap::{Args, Subcommand};
use mmcp_core::manifest::GroupScope;
use mmcp_core::memory::{MemoryFile, parse_sections};
use mmcp_git::{GitBackend, NativeBackend, Rev};
use mmcp_store::home::MmcpHome;
use mmcp_store::{GroupEntry, list_all_memory_files, resolve_group, resolve_memory};
use uuid::Uuid;

use crate::notes::{malformed_frontmatter_notes, render_notes_tail};

// ── Clap surface ────────────────────────────────────────────────

#[derive(Debug, Args)]
#[command(arg_required_else_help = true)]
pub struct MemoryArgs {
    #[command(subcommand)]
    pub cmd: Option<MemoryCommand>,
}

#[derive(Debug, Subcommand)]
pub enum MemoryCommand {
    /// List memories in a group.
    List(ListArgs),
    /// Read a memory's frontmatter + body. Slug-or-UUID positional.
    Read(ReadArgs),
    /// Walk the commit history of a memory.
    Versions(VersionsArgs),
    /// Print the parsed section tree of a memory body (FR-026).
    Sections(SectionsArgs),
    /// Substring search across slug + frontmatter.name.
    Search(SearchArgs),
}

#[derive(Debug, Args)]
pub struct ListArgs {
    /// Target group (UUID or slug).
    pub group: String,
}

#[derive(Debug, Args)]
pub struct ReadArgs {
    /// Target group (UUID or slug).
    pub group: String,

    /// Memory slug or UUID. UUIDs are detected by shape.
    pub addr: String,

    /// Optional revision (branch / tag / commit hex). Defaults
    /// to the repo's HEAD.
    #[arg(long)]
    pub version: Option<String>,
}

#[derive(Debug, Args)]
pub struct VersionsArgs {
    /// Target group (UUID or slug).
    pub group: String,

    /// Memory slug or UUID.
    pub addr: String,
}

#[derive(Debug, Args)]
pub struct SectionsArgs {
    /// Target group (UUID or slug).
    pub group: String,

    /// Memory slug or UUID.
    pub addr: String,
}

#[derive(Debug, Args)]
pub struct SearchArgs {
    /// Substring (case-insensitive) matched against slug and
    /// `name` in frontmatter.
    pub query: String,

    /// Restrict to a single group (UUID or slug).
    #[arg(long)]
    pub group: Option<String>,

    /// Restrict to groups whose scope matches (`global` /
    /// `shared` / `project`).
    #[arg(long)]
    pub scope: Option<String>,

    /// Maximum number of hits. Default 50.
    #[arg(long)]
    pub limit: Option<usize>,
}

// ── Dispatcher ──────────────────────────────────────────────────

pub async fn run(args: MemoryArgs) -> Result<()> {
    match args.cmd {
        // `arg_required_else_help` prints help before this branch
        // when no subcommand is supplied; this arm only fires if
        // a future variant lands without a dispatch update.
        None => unreachable!("clap enforces subcommand presence"),
        Some(MemoryCommand::List(a)) => run_list(a).await,
        Some(MemoryCommand::Read(a)) => run_read(a).await,
        Some(MemoryCommand::Versions(a)) => run_versions(a).await,
        Some(MemoryCommand::Sections(a)) => run_sections(a).await,
        Some(MemoryCommand::Search(a)) => run_search(a).await,
    }
}

// ── Handlers ────────────────────────────────────────────────────

async fn run_list(args: ListArgs) -> Result<()> {
    let home = MmcpHome::discover()?;
    let (backend, groups) = home.init_backend().await?;
    let entry = resolve_group(&groups, &args.group)
        .await
        .map_err(anyhow::Error::from)?;
    let files = list_all_memory_files(&backend, &entry.handle, &Rev::head())
        .await
        .context("listing memory files")?;
    if files.is_empty() {
        println!("group `{}` has no memories", entry.manifest.slug);
        return Ok(());
    }
    println!(
        "{} ({}) — {} memor{}",
        entry.manifest.slug,
        entry.manifest.group_id,
        files.len(),
        if files.len() == 1 { "y" } else { "ies" }
    );
    for file in &files {
        let title = read_title(&backend, &entry, &file.path).await;
        println!("  {} {}  {}", file.slug, short_id(&file.id), title);
    }
    Ok(())
}

async fn run_read(args: ReadArgs) -> Result<()> {
    let home = MmcpHome::discover()?;
    let (backend, groups) = home.init_backend().await?;
    let entry = resolve_group(&groups, &args.group)
        .await
        .map_err(anyhow::Error::from)?;
    let (slug_opt, id_opt) = parse_addr(&args.addr);
    let resolved = resolve_memory(&backend, &entry.handle, slug_opt.as_deref(), id_opt)
        .await
        .map_err(anyhow::Error::from)?;
    let rev = parse_rev(args.version.as_deref());
    let bytes = backend
        .read_file(&entry.handle, &resolved.path, &rev)
        .await
        .map_err(anyhow::Error::from)?;
    let text = std::str::from_utf8(&bytes).context("memory file is not valid UTF-8")?;
    let file = MemoryFile::parse(text)
        .map_err(|e| anyhow::anyhow!("memory frontmatter did not parse: {e}"))?;

    println!(
        "group       : {}",
        entry.manifest.slug
    );
    println!("slug        : {}", resolved.slug);
    println!("id          : {}", resolved.id);
    println!("name        : {}", file.frontmatter.name);
    println!("description : {}", file.frontmatter.description);
    println!("kind        : {}", file.frontmatter.kind.as_str());
    if file.frontmatter.mandatory {
        println!("mandatory   : true");
    }
    if let Some(version) = &file.frontmatter.version {
        println!("version     : {version}");
    }
    if !file.frontmatter.tags.is_empty() {
        println!("tags        : {}", file.frontmatter.tags.join(", "));
    }
    println!();
    println!("{}", file.body);

    let notes = malformed_frontmatter_notes(&resolved.slug, resolved.id, &file);
    render_notes_tail(&notes);
    Ok(())
}

async fn run_versions(args: VersionsArgs) -> Result<()> {
    let home = MmcpHome::discover()?;
    let (backend, groups) = home.init_backend().await?;
    let entry = resolve_group(&groups, &args.group)
        .await
        .map_err(anyhow::Error::from)?;
    let (slug_opt, id_opt) = parse_addr(&args.addr);
    let resolved = resolve_memory(&backend, &entry.handle, slug_opt.as_deref(), id_opt)
        .await
        .map_err(anyhow::Error::from)?;
    let history = backend
        .walk_history(&entry.handle, &resolved.path)
        .await
        .map_err(anyhow::Error::from)?;
    if history.is_empty() {
        println!("no commits touch {}", resolved.path);
        return Ok(());
    }
    for commit in &history {
        // 7-char prefix matches git's default short-hash width.
        let short: String = commit.id.chars().take(7).collect();
        println!(
            "{}  {}  {}",
            short, commit.author_name, commit.subject
        );
    }
    println!("\n{} commit(s)", history.len());
    Ok(())
}

async fn run_sections(args: SectionsArgs) -> Result<()> {
    let home = MmcpHome::discover()?;
    let (backend, groups) = home.init_backend().await?;
    let entry = resolve_group(&groups, &args.group)
        .await
        .map_err(anyhow::Error::from)?;
    let (slug_opt, id_opt) = parse_addr(&args.addr);
    let resolved = resolve_memory(&backend, &entry.handle, slug_opt.as_deref(), id_opt)
        .await
        .map_err(anyhow::Error::from)?;
    let bytes = backend
        .read_file(&entry.handle, &resolved.path, &Rev::head())
        .await
        .map_err(anyhow::Error::from)?;
    let text = std::str::from_utf8(&bytes).context("memory file is not valid UTF-8")?;
    let file = MemoryFile::parse(text)
        .map_err(|e| anyhow::anyhow!("memory frontmatter did not parse: {e}"))?;
    let sections = parse_sections(&file.body)
        .map_err(|e| anyhow::anyhow!("body section parse failed: {e}"))?;
    if sections.is_empty() {
        println!("(no sections — body is empty)");
        return Ok(());
    }
    for section in &sections {
        // Indent by header level so the tree reads at a glance;
        // headings of the same level start at the same column.
        let indent = "  ".repeat(section.level.saturating_sub(1) as usize);
        println!(
            "{indent}{path}  L{level}  [{start}-{end})  {heading}",
            path = section.path,
            level = section.level,
            start = section.line_start,
            end = section.line_end,
            heading = section.heading
        );
    }
    Ok(())
}

async fn run_search(args: SearchArgs) -> Result<()> {
    let needle = args.query.trim().to_lowercase();
    if needle.is_empty() {
        anyhow::bail!("search query must not be empty");
    }
    let limit = args.limit.unwrap_or(50).max(1);

    let home = MmcpHome::discover()?;
    let (backend, groups) = home.init_backend().await?;

    let group_filter = match args.group.as_deref() {
        Some(query) => Some(
            resolve_group(&groups, query)
                .await
                .map_err(anyhow::Error::from)?
                .manifest
                .group_id,
        ),
        None => None,
    };
    let scope_filter = match args.scope.as_deref() {
        None => None,
        Some(s) => Some(parse_scope(s)?),
    };

    let mut hits = 0usize;
    for entry in groups.list().await {
        if hits >= limit {
            break;
        }
        if let Some(target) = group_filter {
            if entry.manifest.group_id != target {
                continue;
            }
        }
        if let Some(target) = scope_filter {
            if entry.manifest.scope != target {
                continue;
            }
        }
        let files = list_all_memory_files(&backend, &entry.handle, &Rev::head())
            .await
            .context("listing memory files")?;
        for file in &files {
            if hits >= limit {
                break;
            }
            let slug_match = file.slug.to_lowercase().contains(&needle);
            // Reading the title gives `name` matching plus a
            // useful display string. Tolerate read failures so a
            // single broken file doesn't kill the whole search.
            let title = read_title(&backend, &entry, &file.path).await;
            let name_match = title.to_lowercase().contains(&needle);
            if slug_match || name_match {
                println!(
                    "{group}/{slug} {short}  {title}",
                    group = entry.manifest.slug,
                    slug = file.slug,
                    short = short_id(&file.id),
                );
                hits += 1;
            }
        }
    }
    if hits == 0 {
        println!("no matches");
    } else {
        println!("\n{hits} hit(s)");
    }
    Ok(())
}

// ── Helpers ─────────────────────────────────────────────────────

/// Return `(slug, id)` from a positional address. UUID shape
/// (36-char hyphenated) routes to the id slot; everything else
/// falls into the slug slot. Lets the CLI match the MCP wire
/// without forcing the operator to pick a flag.
fn parse_addr(addr: &str) -> (Option<String>, Option<Uuid>) {
    if let Ok(id) = Uuid::parse_str(addr) {
        (None, Some(id))
    } else {
        (Some(addr.to_string()), None)
    }
}

/// Mirror of `serve.rs::parse_rev`: 40-char hex → commit,
/// otherwise → branch. Caller passes `None` for "use HEAD".
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

fn parse_scope(s: &str) -> Result<GroupScope> {
    match s {
        "global" => Ok(GroupScope::Global),
        "shared" => Ok(GroupScope::Shared),
        "project" => Ok(GroupScope::Project),
        other => anyhow::bail!("unknown scope `{other}` (expected global / shared / project)"),
    }
}

fn short_id(id: &Uuid) -> String {
    // 8 hex chars is enough to disambiguate within a group at the
    // memory counts we expect, while staying compact in listings.
    format!("{:.8}", id.simple().to_string())
}

/// Read a memory's `frontmatter.name` for listing / search
/// display. Returns `(unreadable)` on failure so a single broken
/// file doesn't kill the listing — the structural error surfaces
/// via `mmcp diagnose` instead.
async fn read_title(backend: &NativeBackend, entry: &GroupEntry, path: &str) -> String {
    let Ok(bytes) = backend.read_file(&entry.handle, path, &Rev::head()).await else {
        return "(unreadable)".into();
    };
    let Ok(text) = std::str::from_utf8(&bytes) else {
        return "(non-utf8)".into();
    };
    match MemoryFile::parse(text) {
        Ok(f) => f.frontmatter.name,
        Err(_) => "(parse error)".into(),
    }
}

