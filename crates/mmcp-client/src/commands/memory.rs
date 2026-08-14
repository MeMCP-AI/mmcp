//! CLI surface for memory CRUD (Slice 2a, read-only ops).
//!
//! Mirrors the read-only memory MCP tools: `list_memories`,
//! `read_memory`, `list_versions`, `read_memory_body_sections`,
//! and `search_memories`. Mutating ops (`write_memory`,
//! `edit_memory`, `edit_memory_body`, `delete_memory`) ship in
//! the next slice.
//!
//! Each subcommand resolves its group + memory through the
//! shared `mmcp_store::resolve_group` / `resolve_memory`
//! primitives (see also: `mmcp_server::routes::mcp::list_memories`
//! for the MCP-side counterpart).
//! The read path also feeds the notes channel via `malformed_frontmatter_notes`;
//! the section reader uses `mmcp_core::memory::body::parse_sections` directly.

use std::io::Read;

use anyhow::{Context, Result};
use clap::{Args, Subcommand};
use mmcp_core::manifest::GroupScope;
use mmcp_core::memory::{
    FrontmatterFormat, MemoryFile, MemoryFrontmatter, MemoryRef, parse_sections,
};
use mmcp_git::{GitBackend, NativeBackend, Rev};
use mmcp_store::home::MmcpHome;
use mmcp_store::{
    AddressingMode, GroupEntry, MemoryEditOp, WriteFileOptions, WriteMemoryOptions, apply_ops,
    delete_file_at_path, list_all_memory_files, parse_creatable_kind, resolve_group,
    resolve_memory, write_file_at_path, write_memory_by_id,
};
use uuid::Uuid;

use crate::commands::import::protected_confirm;
use crate::notes::{id_validation_to_notes, malformed_frontmatter_notes, render_notes_tail};

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
    /// Render the slug-path hierarchy as a tree.
    Tree(TreeArgs),
    /// Read a memory's frontmatter + body. Slug-or-UUID positional.
    Read(ReadArgs),
    /// Walk the commit history of a memory.
    Versions(VersionsArgs),
    /// Print the parsed section tree of a memory body.
    Sections(SectionsArgs),
    /// Substring search across slug + frontmatter.name.
    Search(SearchArgs),
    /// CREATE a new memory in a group with typed metadata.
    Write(WriteArgs),
    /// Apply partial frontmatter / body deltas to an existing memory.
    Edit(EditArgs),
    /// Apply ordered semantic body ops to an existing memory.
    EditBody(EditBodyArgs),
    /// Atomically rewrite a memory's slug path within its group.
    Move(MoveArgs),
    /// Delete a memory by slug or UUID.
    Delete(DeleteArgs),
}

#[derive(Debug, Args)]
pub struct ListArgs {
    /// Target group (UUID or slug).
    pub group: String,

    /// Literal slug-path prefix to filter on.
    /// Pass `feedback` to list every memory whose slug starts with `feedback` or `feedback/...`.
    #[arg(long)]
    pub prefix: Option<String>,

    /// When set, only memories whose slug has at most one path segment beyond `--prefix` are listed.
    /// With no prefix, the same one-segment limit applies to the whole slug.
    /// Default lists every match.
    #[arg(long)]
    pub no_recursive: bool,
}

#[derive(Debug, Args)]
pub struct TreeArgs {
    /// Target group (UUID or slug).
    pub group: String,

    /// Optional literal slug-path prefix.
    /// The tree is rooted at this node so the listing fits the question "what's under feedback/git?".
    #[arg(long)]
    pub prefix: Option<String>,
}

#[derive(Debug, Args)]
pub struct MoveArgs {
    /// Target group (UUID or slug).
    pub group: String,

    /// Source memory address: slug path or UUID.
    /// UUIDs are detected by shape.
    pub addr: String,

    /// New slug path. Multi-segment paths use `/` separators
    /// (e.g. `feedback/git/commit-phase`).
    pub new_slug: String,

    /// Override the git commit message.
    #[arg(long)]
    pub message: Option<String>,
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
pub struct WriteArgs {
    /// Target group (UUID or slug).
    pub group: String,

    /// Memory slug (lowercase + hyphens). Duplicate slugs are
    /// allowed; the server distinguishes by `id`.
    pub slug: String,

    /// Human-readable title.
    #[arg(long)]
    pub name: String,

    /// One-line summary used by listings and relevance inference.
    #[arg(long)]
    pub description: String,

    /// `rule` / `snapshot` / `log` / `reference` / `scratch`.
    #[arg(long)]
    pub kind: String,

    /// Markdown body. Literal string, `@PATH` to read from a file,
    /// or `-` to read from stdin.
    #[arg(long)]
    pub body: String,

    /// Free-form classification tag. Repeat the flag for each.
    #[arg(long = "tag")]
    pub tags: Vec<String>,

    /// Mark this memory as mandatory (must be read once per session).
    #[arg(long)]
    pub mandatory: bool,

    /// Pin a specific UUIDv7 instead of minting one.
    #[arg(long)]
    pub id: Option<String>,

    /// Replace an existing memory at this slug+id. Almost never
    /// the right call; prefer `mmcp memory edit` for partial
    /// updates.
    #[arg(long = "override")]
    pub override_: bool,

    /// Bypass the filename-vs-frontmatter id rejection on a `ByFilename` write.
    /// Drift surfaces as a warn-level note instead of a hard error.
    #[arg(long)]
    pub force: bool,

    /// Pre-confirm a write into a protected group.
    /// Without this flag the command prompts on TTY and refuses
    /// on a non-TTY stdin.
    #[arg(long = "confirm-protected")]
    pub confirm_protected: bool,
}

#[derive(Debug, Args, Default)]
pub struct EditArgs {
    /// Target group (UUID or slug).
    pub group: String,

    /// Memory slug or UUID.
    pub addr: String,

    #[arg(long)]
    pub name: Option<String>,

    #[arg(long)]
    pub description: Option<String>,

    /// New body. Literal, `@PATH`, or `-` for stdin.
    #[arg(long)]
    pub body: Option<String>,

    #[arg(long)]
    pub kind: Option<String>,

    #[arg(long)]
    pub mandatory: Option<bool>,

    /// Tag to add. Repeat the flag.
    #[arg(long = "tag-add")]
    pub tag_add: Vec<String>,

    /// Tag to remove. Repeat the flag.
    #[arg(long = "tag-remove")]
    pub tag_remove: Vec<String>,

    /// Typed cross-reference to add. Format: `<target-uuid>=<commit-hex>`.
    #[arg(long = "ref-add")]
    pub ref_add: Vec<String>,

    /// UUID of a ref target to remove.
    #[arg(long = "ref-remove")]
    pub ref_remove: Vec<String>,

    /// Override the git commit message.
    #[arg(long)]
    pub message: Option<String>,

    /// Bypass the id mismatch rejection.
    #[arg(long)]
    pub force: bool,

    /// Pre-confirm a write into a protected group.
    #[arg(long = "confirm-protected")]
    pub confirm_protected: bool,
}

#[derive(Debug, Args)]
pub struct EditBodyArgs {
    /// Target group (UUID or slug).
    pub group: String,

    /// Memory slug or UUID.
    pub addr: String,

    /// JSON file containing an ordered array of body ops. Pass
    /// `-` to read from stdin. Schema matches
    /// `mmcp_store::MemoryEditOp` (tagged enum, `op` field
    /// names the variant).
    #[arg(long)]
    pub ops: String,

    #[arg(long)]
    pub message: Option<String>,

    #[arg(long)]
    pub force: bool,

    #[arg(long = "confirm-protected")]
    pub confirm_protected: bool,
}

#[derive(Debug, Args)]
pub struct DeleteArgs {
    /// Target group (UUID or slug).
    pub group: String,

    /// Memory slug or UUID.
    pub addr: String,

    /// Override the git commit message.
    #[arg(long)]
    pub message: Option<String>,

    /// Force flag, present for parity with the other write tools.
    /// `delete_memory` does not render new bytes, so the flag has nothing to bypass on the happy path.
    #[arg(long)]
    pub force: bool,

    #[arg(long = "confirm-protected")]
    pub confirm_protected: bool,
}

#[derive(Debug, Args)]
pub struct SearchArgs {
    /// One or more case-insensitive substrings matched against slug and `name` in frontmatter.
    /// Pass multiple positional values for the multi-query form: each positional is a separate query.
    /// Results dedupe by memory UUID,
    /// and the rendered table grows a `matched` column listing which queries hit each row.
    #[arg(num_args = 1..)]
    pub query: Vec<String>,

    /// Restrict to a single group (UUID or slug).
    #[arg(long)]
    pub group: Option<String>,

    /// Restrict to groups whose scope matches (`global` /
    /// `shared` / `project`).
    #[arg(long)]
    pub scope: Option<String>,

    /// Maximum number of hits.
    /// Default 50.
    /// Caps total deduped hits across all queries, not per query.
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
        Some(MemoryCommand::Tree(a)) => run_tree(a).await,
        Some(MemoryCommand::Read(a)) => run_read(a).await,
        Some(MemoryCommand::Versions(a)) => run_versions(a).await,
        Some(MemoryCommand::Sections(a)) => run_sections(a).await,
        Some(MemoryCommand::Search(a)) => run_search(a).await,
        Some(MemoryCommand::Write(a)) => run_write(a).await,
        Some(MemoryCommand::Edit(a)) => run_edit(a).await,
        Some(MemoryCommand::EditBody(a)) => run_edit_body(a).await,
        Some(MemoryCommand::Move(a)) => run_move(a).await,
        Some(MemoryCommand::Delete(a)) => run_delete(a).await,
    }
}

// ── Handlers ────────────────────────────────────────────────────

async fn run_list(args: ListArgs) -> Result<()> {
    let home = MmcpHome::discover()?;
    let (backend, groups) = home.init_backend().await?;
    let entry = resolve_group(&groups, &args.group)
        .await
        .map_err(anyhow::Error::from)?;
    let all_files = list_all_memory_files(&backend, &entry.handle, &Rev::head())
        .await
        .context("listing memory files")?;
    let prefix = args.prefix.as_deref().map(|p| p.trim_end_matches('/'));
    let recursive = !args.no_recursive;
    let files: Vec<_> = all_files
        .into_iter()
        .filter(|f| slug_matches_filter(&f.slug, prefix, recursive))
        .collect();
    if files.is_empty() {
        match prefix {
            Some(p) => println!(
                "group `{}` has no memories under prefix `{}`",
                entry.manifest.slug, p
            ),
            None => println!("group `{}` has no memories", entry.manifest.slug),
        }
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

/// Shared slug-path filter.
/// Returns `true` when `slug` belongs in a listing constrained to `prefix` and the recursion mode.
/// Mirrors the MCP-side helper of the same name (kept in sync by the parity test in `serve.rs`).
fn slug_matches_filter(slug: &str, prefix: Option<&str>, recursive: bool) -> bool {
    let depth = match prefix {
        None | Some("") | Some("/") => slug.split('/').count(),
        Some(p) => {
            if slug == p {
                0
            } else if let Some(rest) = slug.strip_prefix(p)
                && let Some(suffix) = rest.strip_prefix('/')
            {
                suffix.split('/').count()
            } else {
                return false;
            }
        }
    };
    if recursive { true } else { depth <= 1 }
}

async fn run_tree(args: TreeArgs) -> Result<()> {
    let home = MmcpHome::discover()?;
    let (backend, groups) = home.init_backend().await?;
    let entry = resolve_group(&groups, &args.group)
        .await
        .map_err(anyhow::Error::from)?;
    let prefix = args.prefix.as_deref().map(|p| p.trim_end_matches('/'));
    let files = list_all_memory_files(&backend, &entry.handle, &Rev::head())
        .await
        .context("listing memory files")?;
    let mut counts: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    for file in &files {
        if !slug_matches_filter(&file.slug, prefix, true) {
            continue;
        }
        let relative = match prefix {
            None => file.slug.as_str(),
            Some(p) if file.slug == p => "",
            Some(p) => file
                .slug
                .strip_prefix(p)
                .and_then(|rest| rest.strip_prefix('/'))
                .unwrap_or(file.slug.as_str()),
        };
        // Walk the relative slug accumulating one bump per
        // ancestor, so a memory at `git/scope/foo` lights up
        // entries for `git`, `git/scope`, and `git/scope/foo`.
        let mut path = String::new();
        for segment in relative.split('/').filter(|s| !s.is_empty()) {
            if !path.is_empty() {
                path.push('/');
            }
            path.push_str(segment);
            *counts.entry(path.clone()).or_insert(0) += 1;
        }
        if relative.is_empty() {
            *counts.entry(String::new()).or_insert(0) += 1;
        }
    }
    let header = match prefix {
        Some(p) => format!(
            "{} ({}) — tree under `{}`",
            entry.manifest.slug, entry.manifest.group_id, p
        ),
        None => format!(
            "{} ({}) — slug tree",
            entry.manifest.slug, entry.manifest.group_id
        ),
    };
    println!("{header}");
    if counts.is_empty() {
        println!("  (no memories)");
        return Ok(());
    }
    for (path, count) in &counts {
        let depth = if path.is_empty() {
            0
        } else {
            path.matches('/').count() + 1
        };
        let indent = "  ".repeat(depth + 1);
        let label = if path.is_empty() { "." } else { path.as_str() };
        let suffix = if *count == 1 { "memory" } else { "memories" };
        println!("{indent}{label} ({count} {suffix})");
    }
    Ok(())
}

async fn run_move(args: MoveArgs) -> Result<()> {
    let home = MmcpHome::discover()?;
    let (backend, groups) = home.init_backend().await?;
    let entry = resolve_group(&groups, &args.group)
        .await
        .map_err(anyhow::Error::from)?;
    let (slug_opt, id_opt) = parse_addr(&args.addr);
    let author = home.resolve_author();
    // A slug-rewrite move spans source + target slug dirs,
    // so coarsen at the group level just like a feature rename does.
    let _lock_guards = mmcp_store::lock::acquire_chain(&mmcp_store::lock::coarsen_group_chain(
        *entry.manifest.group_id.as_uuid(),
    ))
    .await;
    let outcome = mmcp_store::move_memory_path(
        &backend,
        &entry.handle,
        slug_opt.as_deref(),
        id_opt,
        &args.new_slug,
        &author,
        args.message.as_deref(),
    )
    .await
    .map_err(anyhow::Error::from)?;
    if outcome.commit_id.is_empty() {
        println!(
            "no-op: memory {} already lives at `{}`",
            outcome.id, outcome.new_slug
        );
    } else {
        // 7-char prefix matches git's default short-hash width;
        // operators reading the line don't need the full sha.
        let short: String = outcome.commit_id.chars().take(7).collect();
        println!(
            "moved {} from `{}` to `{}` (commit {short})",
            outcome.id, outcome.old_slug, outcome.new_slug,
        );
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

    println!("group       : {}", entry.manifest.slug);
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
        println!("{}  {}  {}", short, commit.author_name, commit.subject);
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
    if args.query.is_empty() {
        anyhow::bail!("search query must not be empty");
    }
    let mut needles = Vec::with_capacity(args.query.len());
    let mut originals = Vec::with_capacity(args.query.len());
    for q in &args.query {
        let n = q.trim().to_lowercase();
        if n.is_empty() {
            anyhow::bail!("search query must not be empty");
        }
        needles.push(n);
        originals.push(q.clone());
    }
    let multi_mode = needles.len() > 1;
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
        if let Some(target) = group_filter
            && entry.manifest.group_id != target
        {
            continue;
        }
        if let Some(target) = scope_filter
            && entry.manifest.scope != target
        {
            continue;
        }
        let files = list_all_memory_files(&backend, &entry.handle, &Rev::head())
            .await
            .context("listing memory files")?;
        for file in &files {
            if hits >= limit {
                break;
            }
            let slug_lower = file.slug.to_lowercase();
            // Reading the title gives `name` matching plus a
            // useful display string. Tolerate read failures so a
            // single broken file doesn't kill the whole search.
            let title = read_title(&backend, &entry, &file.path).await;
            let name_lower = title.to_lowercase();
            let mut matched: Vec<&str> = Vec::new();
            for (needle, original) in needles.iter().zip(originals.iter()) {
                if slug_lower.contains(needle) || name_lower.contains(needle) {
                    matched.push(original);
                }
            }
            if matched.is_empty() {
                continue;
            }
            if multi_mode {
                println!(
                    "{group}/{slug} {short}  {title}  [matched: {matched}]",
                    group = entry.manifest.slug,
                    slug = file.slug,
                    short = short_id(&file.id),
                    matched = matched.join(", "),
                );
            } else {
                println!(
                    "{group}/{slug} {short}  {title}",
                    group = entry.manifest.slug,
                    slug = file.slug,
                    short = short_id(&file.id),
                );
            }
            hits += 1;
        }
    }
    if hits == 0 {
        println!("no matches");
    } else {
        println!("\n{hits} hit(s)");
    }
    Ok(())
}

async fn run_write(args: WriteArgs) -> Result<()> {
    let body = read_body_input(&args.body)?;
    let kind = parse_creatable_kind(&args.kind).map_err(anyhow::Error::from)?;
    let id = match args.id.as_deref() {
        Some(s) => Uuid::parse_str(s).context("--id is not a valid UUID")?,
        None => Uuid::now_v7(),
    };

    let home = MmcpHome::discover()?;
    let (backend, groups) = home.init_backend().await?;
    let entry = resolve_group(&groups, &args.group)
        .await
        .map_err(anyhow::Error::from)?;

    // Protected-group gate.
    // Same shape as `mmcp import`: TTY prompts, non-TTY refuses unless `--confirm-protected`.
    protected_confirm(&entry, args.confirm_protected)?;

    let file = MemoryFile {
        frontmatter: MemoryFrontmatter::new(args.name, args.description, kind)
            .with_id(id)
            .with_mandatory(args.mandatory)
            .with_tags(args.tags),
        body,
        format: FrontmatterFormat::TomlPlus,
    };
    let rendered = file
        .to_string()
        .map_err(|e| anyhow::anyhow!("render: {e}"))?;

    // Group-level create chain (Process-Shared + Group-Exclusive).
    // The lock layer does not discriminate by kind,
    // so the shared ticket counter and slug-uniqueness invariant both serialise on one scope.
    let _lock_guards = mmcp_store::lock::acquire_chain(&mmcp_store::lock::create_chain(
        *entry.manifest.group_id.as_uuid(),
    ))
    .await;
    let _ = kind;

    let author = home.resolve_author();
    let (commit_id, validation) = write_memory_by_id(
        &backend,
        &entry.handle,
        &args.slug,
        id,
        &rendered,
        &author,
        WriteMemoryOptions {
            override_existing: args.override_,
            addressing_mode: AddressingMode::ByFilename,
            force: args.force,
            ..Default::default()
        },
    )
    .await
    .map_err(anyhow::Error::from)?;

    println!(
        "created memory `{}` (id {}) in group `{}`\n  commit: {}",
        args.slug, id, entry.manifest.slug, commit_id
    );
    let notes = id_validation_to_notes(&validation, &args.slug);
    render_notes_tail(&notes);
    Ok(())
}

async fn run_edit(args: EditArgs) -> Result<()> {
    let home = MmcpHome::discover()?;
    let (backend, groups) = home.init_backend().await?;
    let entry = resolve_group(&groups, &args.group)
        .await
        .map_err(anyhow::Error::from)?;

    protected_confirm(&entry, args.confirm_protected)?;

    let (slug_opt, id_opt) = parse_addr(&args.addr);
    let resolved = resolve_memory(&backend, &entry.handle, slug_opt.as_deref(), id_opt)
        .await
        .map_err(anyhow::Error::from)?;

    let bytes = backend
        .read_file(&entry.handle, &resolved.path, &Rev::head())
        .await
        .map_err(anyhow::Error::from)?;
    let text = String::from_utf8_lossy(&bytes).into_owned();
    let mut file =
        MemoryFile::parse(&text).map_err(|e| anyhow::anyhow!("parsing existing memory: {e}"))?;

    // Apply deltas. Body / frontmatter slot writes are
    // straightforward; tags compose additively with dedup; refs
    // remove-by-target then add (add wins on collision) so
    // repeated calls converge.
    if let Some(body) = args.body {
        file.body = read_body_input(&body)?;
    }
    if let Some(name) = args.name {
        file.frontmatter.name = name;
    }
    if let Some(description) = args.description {
        file.frontmatter.description = description;
    }
    if let Some(kind_str) = args.kind {
        file.frontmatter.kind = parse_creatable_kind(&kind_str).map_err(anyhow::Error::from)?;
    }
    if let Some(mandatory) = args.mandatory {
        file.frontmatter.mandatory = mandatory;
    }
    if !args.tag_add.is_empty() || !args.tag_remove.is_empty() {
        file.frontmatter.tags.extend(args.tag_add);
        file.frontmatter
            .tags
            .retain(|t| !args.tag_remove.contains(t));
        file.frontmatter.tags.sort_unstable();
        file.frontmatter.tags.dedup();
    }
    if !args.ref_remove.is_empty() {
        let removed: Vec<Uuid> = args
            .ref_remove
            .iter()
            .map(|raw| Uuid::parse_str(raw).context("--ref-remove value is not a UUID"))
            .collect::<Result<_>>()?;
        file.frontmatter
            .refs
            .retain(|r| !removed.contains(&r.target));
    }
    if !args.ref_add.is_empty() {
        let to_add = parse_ref_pairs(&args.ref_add)?;
        for new in to_add {
            file.frontmatter.refs.retain(|r| r.target != new.target);
            file.frontmatter.refs.push(new);
        }
    }

    let rendered = file
        .to_string()
        .map_err(|e| anyhow::anyhow!("render: {e}"))?;

    let _lock_guards = mmcp_store::lock::acquire_chain(&mmcp_store::lock::memory_chain(
        *entry.manifest.group_id.as_uuid(),
        resolved.id,
        mmcp_store::lock::LockMode::Exclusive,
    ))
    .await;

    let commit_message = args
        .message
        .unwrap_or_else(|| format!("update memory {}", resolved.slug));
    let author = home.resolve_author();
    let (commit_id, validation) = write_file_at_path(
        &backend,
        &entry.handle,
        &resolved.path,
        &rendered,
        &author,
        WriteFileOptions {
            addressing_mode: resolved.addressing_mode,
            force: args.force,
            message: Some(&commit_message),
        },
    )
    .await
    .map_err(anyhow::Error::from)?;

    println!(
        "updated `{}` (id {})\n  commit: {}",
        resolved.slug, resolved.id, commit_id
    );
    let notes = id_validation_to_notes(&validation, &resolved.slug);
    render_notes_tail(&notes);
    Ok(())
}

async fn run_edit_body(args: EditBodyArgs) -> Result<()> {
    let home = MmcpHome::discover()?;
    let (backend, groups) = home.init_backend().await?;
    let entry = resolve_group(&groups, &args.group)
        .await
        .map_err(anyhow::Error::from)?;

    protected_confirm(&entry, args.confirm_protected)?;

    let (slug_opt, id_opt) = parse_addr(&args.addr);
    let resolved = resolve_memory(&backend, &entry.handle, slug_opt.as_deref(), id_opt)
        .await
        .map_err(anyhow::Error::from)?;

    let ops_json = read_body_input(&args.ops)?;
    let ops: Vec<MemoryEditOp> = serde_json::from_str(&ops_json).context("parsing --ops JSON")?;

    let bytes = backend
        .read_file(&entry.handle, &resolved.path, &Rev::head())
        .await
        .map_err(anyhow::Error::from)?;
    let text = String::from_utf8_lossy(&bytes).into_owned();
    let mut file =
        MemoryFile::parse(&text).map_err(|e| anyhow::anyhow!("parsing existing memory: {e}"))?;

    let new_body = apply_ops(&file.body, &ops).map_err(anyhow::Error::from)?;
    file.body = new_body;
    let rendered = file
        .to_string()
        .map_err(|e| anyhow::anyhow!("render: {e}"))?;

    let _lock_guards = mmcp_store::lock::acquire_chain(&mmcp_store::lock::memory_chain(
        *entry.manifest.group_id.as_uuid(),
        resolved.id,
        mmcp_store::lock::LockMode::Exclusive,
    ))
    .await;

    let commit_message = args
        .message
        .unwrap_or_else(|| format!("update memory {}", resolved.slug));
    let author = home.resolve_author();
    let (commit_id, validation) = write_file_at_path(
        &backend,
        &entry.handle,
        &resolved.path,
        &rendered,
        &author,
        WriteFileOptions {
            addressing_mode: resolved.addressing_mode,
            force: args.force,
            message: Some(&commit_message),
        },
    )
    .await
    .map_err(anyhow::Error::from)?;

    println!(
        "edited body of `{}` (id {})\n  commit: {}",
        resolved.slug, resolved.id, commit_id
    );
    let notes = id_validation_to_notes(&validation, &resolved.slug);
    render_notes_tail(&notes);
    Ok(())
}

async fn run_delete(args: DeleteArgs) -> Result<()> {
    let home = MmcpHome::discover()?;
    let (backend, groups) = home.init_backend().await?;
    let entry = resolve_group(&groups, &args.group)
        .await
        .map_err(anyhow::Error::from)?;

    protected_confirm(&entry, args.confirm_protected)?;

    let (slug_opt, id_opt) = parse_addr(&args.addr);
    let resolved = resolve_memory(&backend, &entry.handle, slug_opt.as_deref(), id_opt)
        .await
        .map_err(anyhow::Error::from)?;

    // Read once to learn the kind for the lock chain.
    let kind = match backend
        .read_file(&entry.handle, &resolved.path, &Rev::head())
        .await
    {
        Ok(bytes) => MemoryFile::parse(&String::from_utf8_lossy(&bytes))
            .map(|f| f.frontmatter.kind)
            .unwrap_or(mmcp_core::memory::MemoryKind::Reference),
        Err(_) => mmcp_core::memory::MemoryKind::Reference,
    };

    let _lock_guards = mmcp_store::lock::acquire_chain(&mmcp_store::lock::memory_chain(
        *entry.manifest.group_id.as_uuid(),
        resolved.id,
        mmcp_store::lock::LockMode::Exclusive,
    ))
    .await;
    let _ = kind;

    // `--force` is accepted on the wire for parity with the other
    // write tools; on delete it has nothing to bypass since we
    // render no new bytes. Bind to underscore so the arg stays
    // visible in the surface.
    let _force = args.force;

    let commit_message = args
        .message
        .unwrap_or_else(|| format!("delete memory {}", resolved.slug));
    let author = home.resolve_author();
    let commit_id = delete_file_at_path(
        &backend,
        &entry.handle,
        &resolved.path,
        &author,
        Some(&commit_message),
    )
    .await
    .map_err(anyhow::Error::from)?;

    println!(
        "deleted `{}` (id {})\n  commit: {}",
        resolved.slug, resolved.id, commit_id
    );
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

/// Read body / ops input. Literal returns as-is; a `@PATH`
/// prefix reads from a file; `-` reads from stdin. Lets every
/// write subcommand accept the same three input shapes without
/// re-implementing the dispatch.
fn read_body_input(raw: &str) -> Result<String> {
    if raw == "-" {
        let mut buf = String::new();
        std::io::stdin()
            .read_to_string(&mut buf)
            .context("reading body from stdin")?;
        Ok(buf)
    } else if let Some(path) = raw.strip_prefix('@') {
        std::fs::read_to_string(path).with_context(|| format!("reading body from {path}"))
    } else {
        Ok(raw.to_string())
    }
}

/// Parse `--ref-add target=commit` pairs. `target` must be a
/// UUID; `commit` must be 40-char lowercase hex (matches the
/// invariant the store enforces on `MemoryRef`).
fn parse_ref_pairs(raws: &[String]) -> Result<Vec<MemoryRef>> {
    raws.iter()
        .map(|raw| {
            let (target, commit) = raw.split_once('=').ok_or_else(|| {
                anyhow::anyhow!("--ref-add expects `<target-uuid>=<commit-hex>`, got `{raw}`")
            })?;
            let target = Uuid::parse_str(target.trim())
                .with_context(|| format!("ref target `{target}` is not a UUID"))?;
            let commit = commit.trim().to_string();
            if commit.len() != 40 || !commit.chars().all(|c| c.is_ascii_hexdigit()) {
                anyhow::bail!("ref commit `{commit}` must be 40-char lowercase hex");
            }
            Ok(MemoryRef { target, commit })
        })
        .collect()
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
        other => {
            anyhow::bail!("unknown scope `{other}` (expected global / shared / project)")
        }
    }
}

fn short_id(id: &Uuid) -> String {
    // 8 hex chars is enough to disambiguate within a group at the
    // memory counts we expect, while staying compact in listings.
    format!("{:.8}", id.simple().to_string())
}

/// Read a memory's `frontmatter.name` for listing / search display.
/// Returns `(unreadable)` on failure so a single broken file doesn't kill the listing:
/// the structural error surfaces via `mmcp diagnose` instead.
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
