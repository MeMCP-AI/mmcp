//! `mmcp init claude`: generate, append to, or convert CLAUDE.md.
//!
//! The CLI and the MCP `init_claude` tool share this module's core
//! (`plan` + `execute`). Interactive prompts and process-exit logic
//! live only in [`run`], so the MCP side can reuse the pure pieces
//! without pulling stdin/TTY behavior into tool calls.

use std::fmt;
use std::io::{self, IsTerminal};
use std::path::{Path, PathBuf};
use std::process::Command as StdCommand;

use anyhow::{Context, Result, anyhow, bail};
use inquire::{InquireError, Select};
use mmcp_core::memory::{FrontmatterFormat, MemoryFile, MemoryFrontmatter, MemoryKind};

use mmcp_store::StoreError;
use mmcp_store::config::{self, PROJECT_MANIFEST};
use mmcp_store::home::{MmcpHome, ResolvedAuthor};

mod fence;
mod init_claude_action;
mod init_claude_conflict;
mod notes;
mod partial_fence_error;

pub use fence::{Fence, scan_fence};
pub use init_claude_action::{InitClaudeAction, action_wire};
pub use init_claude_conflict::InitClaudeConflict;
pub use notes::claude_md_notes;
pub use partial_fence_error::PartialFenceError;

// Public CLI entry point

/// Flags accepted by `mmcp init claude`.
#[derive(Debug, Clone, clap::Args)]
pub struct ClaudeArgs {
    /// Overwrite CLAUDE.md with a fresh mmcp stub.
    #[arg(long, group = "action")]
    pub r#override: bool,

    /// Split existing CLAUDE.md into typed project memories, then
    /// replace the file with the stub.
    #[arg(long, group = "action")]
    pub convert: bool,

    /// Insert (or replace) the mmcp-managed block inside an existing
    /// CLAUDE.md without touching content outside the fence.
    #[arg(long, group = "action")]
    pub append: bool,

    /// Always write a `.bak` copy before modifying CLAUDE.md. Default
    /// policy: back up only when the file is untracked or dirty.
    #[arg(long, conflicts_with = "no_backup")]
    pub backup: bool,

    /// Never write a `.bak` copy.
    #[arg(long, conflicts_with = "backup")]
    pub no_backup: bool,

    /// Print the intended action and exit without touching the file
    /// or writing any memories.
    #[arg(long)]
    pub dry_run: bool,

    /// Skip safety prompts. Required on non-TTY stdin when the file
    /// state would otherwise prompt.
    #[arg(long)]
    pub force: bool,

    /// Path to the CLAUDE.md file to manage. Defaults to `./CLAUDE.md`.
    #[arg(long, default_value = CLAUDE_MD_FILE_NAME)]
    pub path: PathBuf,
}

/// CLI entry. Resolves a plan, runs safety prompts if needed, then
/// executes the plan (or prints it on `--dry-run`).
pub async fn run(args: ClaudeArgs) -> Result<()> {
    let cwd = std::env::current_dir().context("reading current working directory")?;
    let home = MmcpHome::discover()?;
    let author = home.resolve_author();

    let action = resolve_action(&args, io::stdin().is_terminal())?;
    let state = inspect(&args.path);

    // Resolve backup policy and confirmation up front so the caller
    // sees one question, not three.
    let mut backup = default_backup(&args, &state);
    let conflict = resolve_conflict(&args, &state, io::stdin().is_terminal())?;
    match conflict {
        ConflictChoice::Cancel => {
            eprintln!("Cancelled; CLAUDE.md is unchanged.");
            return Ok(());
        }
        ConflictChoice::BackupOverride => backup = true,
        ConflictChoice::Override | ConflictChoice::NotApplicable => {}
    }

    let plan = ClaudePlan {
        action,
        backup,
        dry_run: args.dry_run,
        path: args.path.clone(),
        state,
        cwd,
    };

    if plan.dry_run {
        print_plan(&plan);
        return Ok(());
    }

    let report = execute(&plan, &home, &author).await?;
    print_report(&report);
    Ok(())
}

// Shared types

/// Resolved intent for the command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Override,
    Append,
    Convert,
}

/// Observed state of the target file, relative to git.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileState {
    Missing,
    Untracked,
    TrackedClean,
    TrackedDirty,
}

impl FileState {
    pub fn is_conflict(&self) -> bool {
        matches!(self, FileState::Untracked | FileState::TrackedDirty)
    }

    pub fn as_wire_str(&self) -> &'static str {
        match self {
            FileState::Missing => "missing",
            FileState::Untracked => "untracked",
            FileState::TrackedClean => "tracked_clean",
            FileState::TrackedDirty => "tracked_dirty",
        }
    }
}

/// How a dirty/untracked conflict was (or should be) resolved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictChoice {
    /// File state does not trigger a conflict (missing + override, or clean).
    NotApplicable,
    /// Overwrite without backup.
    Override,
    /// Write a `.bak` first, then overwrite.
    BackupOverride,
    /// Do nothing.
    Cancel,
}

/// Plan for the command. Pure data, no I/O.
#[derive(Debug, Clone)]
pub struct ClaudePlan {
    pub action: Action,
    pub backup: bool,
    pub dry_run: bool,
    pub path: PathBuf,
    pub state: FileState,
    pub cwd: PathBuf,
}

/// Post-execution report.
#[derive(Debug, Clone)]
pub struct ClaudeReport {
    pub action: Action,
    pub state_before: FileState,
    pub backup_path: Option<PathBuf>,
    pub wrote: Option<PathBuf>,
    pub memories_created: Vec<MemoryCreated>,
}

#[derive(Debug, Clone)]
pub struct MemoryCreated {
    pub slug: String,
    pub commit_id: String,
    pub source_section: String,
}

// File names, managed block and fence markers the MCP side also reaches for

/// File name of a Claude Code instruction file.
pub const CLAUDE_MD_FILE_NAME: &str = "CLAUDE.md";

/// Claude Code's user-level directory, under the user's home.
const USER_CLAUDE_DIR: &str = ".claude";

/// The user-level instruction file, `~/.claude/CLAUDE.md`.
///
/// # Errors
///
/// [`StoreError::HomeDirUnresolved`] when the user's home cannot be resolved.
pub fn user_claude_md_path() -> Result<PathBuf, StoreError> {
    Ok(mmcp_store::resolve_user_home()?
        .join(USER_CLAUDE_DIR)
        .join(CLAUDE_MD_FILE_NAME))
}

/// Version tag of the mmcp-managed block, carried by both fence markers.
pub const BLOCK_VERSION: &str = "v2";

/// Opening of a begin marker, whatever its version.
const BEGIN_MARKER_PREFIX: &str = "<!-- mmcp:begin ";

/// Opening of an end marker, whatever its version.
const END_MARKER_PREFIX: &str = "<!-- mmcp:end ";

/// Closing of either fence marker.
const MARKER_SUFFIX: &str = " -->";

/// Begin marker of the current block version.
#[must_use]
pub fn begin_marker() -> String {
    format!("{BEGIN_MARKER_PREFIX}{BLOCK_VERSION}{MARKER_SUFFIX}")
}

/// End marker of the current block version.
#[must_use]
pub fn end_marker() -> String {
    format!("{END_MARKER_PREFIX}{BLOCK_VERSION}{MARKER_SUFFIX}")
}

/// Body of the managed block between its two fence markers.
/// A sentence followed by another in the same paragraph ends with a two-space hard break.
const BLOCK_BODY: &str = concat!(
    "<!-- Managed by mmcp, regenerated by `mmcp init claude`. -->\n",
    "<!-- Rules are memories, edited with `mmcp__write_memory`, never here. -->\n",
    "\n",
    "## mmcp is the source of truth\n",
    "\n",
    "mmcp holds every rule, convention and note of this project.  \n",
    "The operator may waive the procedure below for a session.  \n",
    "An operator instruction outranks this file.\n",
    "\n",
    "When mmcp is unreachable, the work in flight continues on the rules already read, review and audit included.  \n",
    "A task whose rules were not read does not start until mmcp is back.  \n",
    "Write every report meant for mmcp into a temporary Claude Code memory of the project.  \n",
    "Replay it into mmcp once the server is back.\n",
    "\n",
    "## What each memory kind is worth\n",
    "\n",
    "A rule is absolute. It changes only after the operator's review and approval.  \n",
    "A feedback memory, under the `feedback/` prefix, is a rule candidate and a suggestion. It never outranks a rule.  \n",
    "Every other kind is a record: log, incident, scratch, snapshot, reference, feature, issue. A record is trusted as written.\n",
    "\n",
    "## When the rules are read\n",
    "\n",
    "### At the session start\n",
    "\n",
    "Call `mmcp__bootstrap_context` before any other tool call or file write.  \n",
    "Read every rule it points at, in full.\n",
    "\n",
    "### After a context compaction\n",
    "\n",
    "Read the rules again.  \n",
    "A compaction invalidates the rules only. The rest of its summary stays trusted.\n",
    "\n",
    "### Between those two moments\n",
    "\n",
    "Read the memory governing the subject at hand before working on it.\n",
);

/// The mmcp-managed block, begin marker through end marker, without a trailing newline.
/// Override, append and the staleness check all use this one render.
#[must_use]
pub fn render_block() -> String {
    format!("{}\n{BLOCK_BODY}{}", begin_marker(), end_marker())
}

// Action resolution

fn resolve_action(args: &ClaudeArgs, is_tty: bool) -> Result<Action> {
    match (args.r#override, args.convert, args.append) {
        (true, false, false) => Ok(Action::Override),
        (false, true, false) => Ok(Action::Convert),
        (false, false, true) => Ok(Action::Append),
        (false, false, false) => {
            if is_tty {
                prompt_action_interactive()
            } else {
                bail!(
                    "no action flag given on a non-TTY invocation; pass one of --override, --convert, --append"
                )
            }
        }
        _ => bail!("--override, --convert, --append are mutually exclusive"),
    }
}

/// Menu options for the top-level action prompt. `Cancel` is rendered
/// by `inquire` alongside the real actions; user pressing ESC also
/// maps to a cancel (via `InquireError::OperationCanceled`).
#[derive(Clone, Copy, PartialEq, Eq)]
enum ActionChoice {
    Override,
    Append,
    Convert,
    Cancel,
}

impl fmt::Display for ActionChoice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            ActionChoice::Override => "override: replace CLAUDE.md with a fresh mmcp stub",
            ActionChoice::Append => "append: insert/replace the mmcp block inside CLAUDE.md",
            ActionChoice::Convert => "convert: split CLAUDE.md into typed memories, then stub",
            ActionChoice::Cancel => "cancel",
        };
        f.write_str(s)
    }
}

fn prompt_action_interactive() -> Result<Action> {
    let options = vec![
        ActionChoice::Override,
        ActionChoice::Append,
        ActionChoice::Convert,
        ActionChoice::Cancel,
    ];
    match Select::new("mmcp init claude: pick an action", options).prompt() {
        Ok(ActionChoice::Override) => Ok(Action::Override),
        Ok(ActionChoice::Append) => Ok(Action::Append),
        Ok(ActionChoice::Convert) => Ok(Action::Convert),
        Ok(ActionChoice::Cancel)
        | Err(InquireError::OperationCanceled)
        | Err(InquireError::OperationInterrupted) => bail!("cancelled"),
        Err(e) => Err(anyhow!(e).context("reading action choice")),
    }
}

// File state detection

/// Inspect the file on disk and report its state relative to git.
pub fn inspect(path: &Path) -> FileState {
    if !path.exists() {
        return FileState::Missing;
    }
    match git_status_of(path) {
        GitStatus::NotTracked => FileState::Untracked,
        GitStatus::Clean => FileState::TrackedClean,
        GitStatus::Dirty => FileState::TrackedDirty,
        GitStatus::OutsideRepo => FileState::Untracked,
    }
}

enum GitStatus {
    NotTracked,
    Clean,
    Dirty,
    OutsideRepo,
}

fn git_status_of(path: &Path) -> GitStatus {
    // Resolve `git` via the same env shim the native backend uses so
    // `MMCP_GIT_BIN` overrides are honored here too.
    let bin = std::env::var_os("MMCP_GIT_BIN").unwrap_or_else(|| "git".into());
    let output = StdCommand::new(bin)
        .arg("status")
        .arg("--porcelain=v1")
        .arg("--")
        .arg(path)
        .output();
    let Ok(out) = output else {
        return GitStatus::OutsideRepo;
    };
    if !out.status.success() {
        return GitStatus::OutsideRepo;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    if text.trim().is_empty() {
        return GitStatus::Clean;
    }
    let first_line = text.lines().next().unwrap_or("");
    if first_line.starts_with("??") {
        GitStatus::NotTracked
    } else {
        GitStatus::Dirty
    }
}

// Safety prompts

fn default_backup(args: &ClaudeArgs, state: &FileState) -> bool {
    if args.backup {
        return true;
    }
    if args.no_backup {
        return false;
    }
    // Default: back up iff the file would lose uncommitted work.
    state.is_conflict()
}

fn resolve_conflict(args: &ClaudeArgs, state: &FileState, is_tty: bool) -> Result<ConflictChoice> {
    if !state.is_conflict() {
        return Ok(ConflictChoice::NotApplicable);
    }
    if args.force {
        return Ok(if args.no_backup {
            ConflictChoice::Override
        } else {
            ConflictChoice::BackupOverride
        });
    }
    if !is_tty {
        bail!(
            "CLAUDE.md is {} and --force was not given; pass --force (and --backup or --no-backup if desired) on non-TTY invocations",
            state.as_wire_str()
        );
    }
    prompt_conflict_interactive(*state)
}

/// Menu options for the dirty/untracked conflict prompt. Kept
/// separate from [`ConflictChoice`] because the menu has no
/// `NotApplicable` variant: the caller only prompts when the file
/// actually conflicts.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ConflictMenu {
    BackupOverride,
    Override,
    Cancel,
}

impl fmt::Display for ConflictMenu {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            ConflictMenu::BackupOverride => "backup (.bak) and override  (recommended)",
            ConflictMenu::Override => "override without backup  (dangerous)",
            ConflictMenu::Cancel => "cancel",
        };
        f.write_str(s)
    }
}

fn prompt_conflict_interactive(state: FileState) -> Result<ConflictChoice> {
    let message = match state {
        FileState::Untracked => "CLAUDE.md is untracked (not committed to git). How to proceed?",
        FileState::TrackedDirty => "CLAUDE.md has uncommitted changes. How to proceed?",
        _ => unreachable!(),
    };
    let options = vec![
        ConflictMenu::BackupOverride,
        ConflictMenu::Override,
        ConflictMenu::Cancel,
    ];
    match Select::new(message, options).prompt() {
        Ok(ConflictMenu::BackupOverride) => Ok(ConflictChoice::BackupOverride),
        Ok(ConflictMenu::Override) => Ok(ConflictChoice::Override),
        Ok(ConflictMenu::Cancel)
        | Err(InquireError::OperationCanceled)
        | Err(InquireError::OperationInterrupted) => Ok(ConflictChoice::Cancel),
        Err(e) => Err(anyhow!(e).context("reading conflict choice")),
    }
}

// Execution

pub async fn execute(
    plan: &ClaudePlan,
    home: &MmcpHome,
    author: &ResolvedAuthor,
) -> Result<ClaudeReport> {
    // Validate action-vs-state combos that only make sense with
    // certain preconditions.
    if matches!(plan.action, Action::Append | Action::Convert)
        && matches!(plan.state, FileState::Missing)
    {
        bail!(
            "cannot {} a missing CLAUDE.md; run with --override to write the stub first",
            match plan.action {
                Action::Append => "append to",
                Action::Convert => "convert",
                Action::Override => unreachable!(),
            }
        );
    }

    let backup_path = if plan.backup && !matches!(plan.state, FileState::Missing) {
        Some(backup_file(&plan.path)?)
    } else {
        None
    };

    let (wrote, memories_created) = match plan.action {
        Action::Override => {
            std::fs::write(&plan.path, stub_contents())
                .with_context(|| format!("writing stub to {}", plan.path.display()))?;
            (Some(plan.path.clone()), Vec::new())
        }
        Action::Append => {
            let existing = std::fs::read_to_string(&plan.path)
                .with_context(|| format!("reading {}", plan.path.display()))?;
            let new = splice_block(&existing)
                .with_context(|| format!("splicing the mmcp block into {}", plan.path.display()))?;
            std::fs::write(&plan.path, new)
                .with_context(|| format!("writing {}", plan.path.display()))?;
            (Some(plan.path.clone()), Vec::new())
        }
        Action::Convert => {
            let existing = std::fs::read_to_string(&plan.path)
                .with_context(|| format!("reading {}", plan.path.display()))?;
            let (created, wrote) =
                convert_and_write(&existing, &plan.path, &plan.cwd, home, author).await?;
            (wrote, created)
        }
    };

    Ok(ClaudeReport {
        action: plan.action,
        state_before: plan.state,
        backup_path,
        wrote,
        memories_created,
    })
}

fn backup_file(path: &Path) -> Result<PathBuf> {
    let mut bak = path.as_os_str().to_owned();
    bak.push(".bak");
    let bak = PathBuf::from(bak);
    std::fs::copy(path, &bak)
        .with_context(|| format!("backing {} to {}", path.display(), bak.display()))?;
    Ok(bak)
}

// Stub content (override mode)

/// The generated CLAUDE.md: the title, then the managed block alone.
#[must_use]
pub fn stub_contents() -> String {
    format!("# CLAUDE.md\n\n{}\n", render_block())
}

// Append mode (fenced mmcp block)

/// Insert the mmcp-managed block into `existing`, or replace a fenced region of any version in place.
/// Every byte outside the fence is preserved.
///
/// # Errors
///
/// [`PartialFenceError`] when `existing` carries a begin marker or an end marker without its counterpart.
pub fn splice_block(existing: &str) -> Result<String, PartialFenceError> {
    let block = render_block();

    if let Some(fence) = scan_fence(existing)? {
        let mut out = String::with_capacity(existing.len() - fence.range.len() + block.len());
        out.push_str(&existing[..fence.range.start]);
        out.push_str(&block);
        out.push_str(&existing[fence.range.end..]);
        return Ok(out);
    }

    // No fence: append the block after a blank separator line.
    let mut out = existing.to_string();
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out.push('\n');
    out.push_str(&block);
    out.push('\n');
    Ok(out)
}

// Convert mode

async fn convert_and_write(
    existing: &str,
    path: &Path,
    cwd: &Path,
    home: &MmcpHome,
    author: &ResolvedAuthor,
) -> Result<(Vec<MemoryCreated>, Option<PathBuf>)> {
    let sections = split_sections(existing);
    if sections.is_empty() {
        bail!("CLAUDE.md has no convertible content; run --override to write the stub instead");
    }

    // Target group: the project's own group from .mmcp.toml.
    let root = config::find_project_root(cwd).ok_or_else(|| {
        anyhow!(
            "convert requires a project with a {PROJECT_MANIFEST} at the cwd or an ancestor directory; run `mmcp init` first"
        )
    })?;
    let project_cfg = config::load(&root)
        .with_context(|| format!("loading project config from {}", root.display()))?;
    let project_uuid = *project_cfg.project_uuid.as_uuid();

    let (backend, groups) = home.init_backend().await?;
    let group_id = mmcp_core::id::GroupId::from_uuid(project_uuid);
    let entry = groups.get(&group_id).await.ok_or_else(|| {
        anyhow!(
            "project group {project_uuid} is not present in the local mirror; push or clone the project repo first"
        )
    })?;

    // Announce the plan before writing anything.
    eprintln!(
        "convert will write {} memory(ies) into group `{}` ({}):",
        sections.len(),
        entry.manifest.slug,
        project_uuid
    );
    for section in &sections {
        eprintln!(
            "  - {} → {} ({})",
            section.title,
            section.slug,
            section.kind.as_str()
        );
    }

    let mut created = Vec::with_capacity(sections.len());
    for section in sections {
        let file = MemoryFile {
            frontmatter: MemoryFrontmatter::new(
                section.title.clone(),
                section.description.clone(),
                section.kind,
            )
            .with_mandatory(section.mandatory)
            .with_tags(section.tags.clone()),
            body: section.body.clone(),
            format: FrontmatterFormat::TomlPlus,
        };
        let rendered = file
            .to_string()
            .map_err(|e| anyhow!("rendering frontmatter for `{}`: {e}", section.slug))?;
        let path =
            mmcp_core::conventions::memory_path(&section.slug, mmcp_core::id::MemoryId::new());
        let (commit_id, _validation) = mmcp_store::write_file_at_path(
            &backend,
            &entry.handle,
            &path,
            &rendered,
            author,
            mmcp_store::WriteFileOptions {
                addressing_mode: mmcp_store::AddressingMode::ByFilename,
                force: false,
                message: Some(&format!("convert CLAUDE.md section: {}", section.title)),
            },
        )
        .await
        .with_context(|| format!("writing memory {}", section.slug))?;
        created.push(MemoryCreated {
            slug: section.slug,
            commit_id,
            source_section: section.title,
        });
    }

    std::fs::write(path, stub_contents())
        .with_context(|| format!("writing stub to {}", path.display()))?;

    Ok((created, Some(path.to_path_buf())))
}

/// Section of CLAUDE.md that converts to one memory.
#[derive(Debug, Clone)]
struct Section {
    title: String,
    slug: String,
    description: String,
    kind: MemoryKind,
    mandatory: bool,
    tags: Vec<String>,
    body: String,
}

/// Split CLAUDE.md on top-level (`##`) headers. Content before the
/// first H2 becomes a `Preamble` section.
fn split_sections(input: &str) -> Vec<Section> {
    let mut sections: Vec<(String, String)> = Vec::new(); // (title, body)
    let mut current_title = String::from("Preamble");
    let mut current_body = String::new();
    for line in input.lines() {
        if let Some(rest) = line.strip_prefix("## ") {
            // Flush previous section if it has content.
            if !current_body.trim().is_empty() {
                sections.push((
                    std::mem::take(&mut current_title),
                    std::mem::take(&mut current_body),
                ));
            } else {
                // Discard empty preamble.
                current_body.clear();
            }
            current_title = rest.trim().to_string();
            continue;
        }
        current_body.push_str(line);
        current_body.push('\n');
    }
    if !current_body.trim().is_empty() {
        sections.push((current_title, current_body));
    }

    let mut out = Vec::with_capacity(sections.len());
    let mut used_slugs: std::collections::HashSet<String> = std::collections::HashSet::new();
    for (title, body) in sections {
        let base_slug = slugify(&title);
        let slug = uniquify(&base_slug, &mut used_slugs);
        let description = first_paragraph(&body);
        let upper = body.to_uppercase();
        let is_rule =
            upper.contains("MUST") || upper.contains("MANDATORY") || upper.contains("NEVER");
        let kind = if is_rule {
            MemoryKind::Rule
        } else {
            MemoryKind::Reference
        };
        let mandatory = is_rule;
        let mut tags = vec!["imported".to_string(), "claude-md".to_string()];
        for h3 in collect_h3_tags(&body) {
            tags.push(h3);
        }
        out.push(Section {
            title,
            slug,
            description,
            kind,
            mandatory,
            tags,
            body: body.trim_end().to_string(),
        });
    }
    out
}

fn slugify(text: &str) -> String {
    // `slug::slugify` handles Unicode normalization, hyphen
    // collapsing, and edge trimming. It returns an empty string for
    // input that contains no slug-safe characters (e.g. `"!!!"`), so
    // fall back to the placeholder `section` to keep `imported-`
    // slugs well-formed under validate_slug_segment.
    let base = slug::slugify(text);
    let base = if base.is_empty() {
        "section"
    } else {
        base.as_str()
    };
    format!("imported-{base}")
}

fn uniquify(base: &str, used: &mut std::collections::HashSet<String>) -> String {
    if used.insert(base.to_string()) {
        return base.to_string();
    }
    let mut n = 2u32;
    loop {
        let candidate = format!("{base}-{n}");
        if used.insert(candidate.clone()) {
            return candidate;
        }
        n += 1;
    }
}

fn first_paragraph(body: &str) -> String {
    let mut buf = String::new();
    for line in body.lines() {
        if line.trim().is_empty() {
            if !buf.is_empty() {
                break;
            }
            continue;
        }
        if !buf.is_empty() {
            buf.push(' ');
        }
        buf.push_str(line.trim());
    }
    if buf.len() > 160 {
        buf.truncate(157);
        buf.push_str("...");
    }
    if buf.is_empty() {
        buf.push_str("Imported from CLAUDE.md");
    }
    buf
}

fn collect_h3_tags(body: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in body.lines() {
        if let Some(rest) = line.strip_prefix("### ") {
            let tag = slugify(rest.trim());
            // Drop the "imported-" prefix slugify added so tags are clean.
            let tag = tag.trim_start_matches("imported-").to_string();
            if !tag.is_empty() {
                out.push(tag);
            }
        }
    }
    out
}

// Reporting

fn print_plan(plan: &ClaudePlan) {
    eprintln!("--dry-run: would apply plan:");
    eprintln!("  action:   {:?}", plan.action);
    eprintln!("  path:     {}", plan.path.display());
    eprintln!("  state:    {}", plan.state.as_wire_str());
    eprintln!("  backup:   {}", plan.backup);
}

fn print_report(report: &ClaudeReport) {
    eprintln!("{:?} applied:", report.action);
    eprintln!("  state before: {}", report.state_before.as_wire_str());
    if let Some(bak) = &report.backup_path {
        eprintln!("  backup:       {}", bak.display());
    }
    if let Some(wrote) = &report.wrote {
        eprintln!("  wrote:        {}", wrote.display());
    }
    for mem in &report.memories_created {
        eprintln!(
            "  memory:       {} ({})  ← section `{}`",
            mem.slug, mem.commit_id, mem.source_section
        );
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use mmcp_git::GitBackend;
    use mmcp_store::config::PROJECT_MANIFEST;

    fn default_args() -> ClaudeArgs {
        ClaudeArgs {
            r#override: false,
            convert: false,
            append: false,
            backup: false,
            no_backup: false,
            dry_run: false,
            force: false,
            path: PathBuf::from("CLAUDE.md"),
        }
    }

    #[test]
    fn resolve_action_uses_explicit_flag_when_set() {
        let mut args = default_args();
        args.r#override = true;
        assert_eq!(resolve_action(&args, false).unwrap(), Action::Override);
    }

    #[test]
    fn resolve_action_rejects_conflicting_flags() {
        let mut args = default_args();
        args.r#override = true;
        args.convert = true;
        assert!(resolve_action(&args, false).is_err());
    }

    #[test]
    fn resolve_action_errors_on_non_tty_with_no_flags() {
        let args = default_args();
        let err = resolve_action(&args, false).unwrap_err();
        assert!(err.to_string().contains("non-TTY"));
    }

    #[test]
    fn default_backup_is_true_for_dirty_and_false_for_clean() {
        let args = default_args();
        assert!(default_backup(&args, &FileState::Untracked));
        assert!(default_backup(&args, &FileState::TrackedDirty));
        assert!(!default_backup(&args, &FileState::TrackedClean));
        assert!(!default_backup(&args, &FileState::Missing));
    }

    #[test]
    fn default_backup_honors_explicit_flag() {
        let mut args = default_args();
        args.backup = true;
        assert!(default_backup(&args, &FileState::TrackedClean));
        args.backup = false;
        args.no_backup = true;
        assert!(!default_backup(&args, &FileState::TrackedDirty));
    }

    /// The operator-approved v2 block, fence markers included.
    /// Every sentence followed by another in the same paragraph ends with a two-space hard break.
    const APPROVED_V2_BLOCK: &str = concat!(
        "<!-- mmcp:begin v2 -->\n",
        "<!-- Managed by mmcp, regenerated by `mmcp init claude`. -->\n",
        "<!-- Rules are memories, edited with `mmcp__write_memory`, never here. -->\n",
        "\n",
        "## mmcp is the source of truth\n",
        "\n",
        "mmcp holds every rule, convention and note of this project.  \n",
        "The operator may waive the procedure below for a session.  \n",
        "An operator instruction outranks this file.\n",
        "\n",
        "When mmcp is unreachable, the work in flight continues on the rules already read, review and audit included.  \n",
        "A task whose rules were not read does not start until mmcp is back.  \n",
        "Write every report meant for mmcp into a temporary Claude Code memory of the project.  \n",
        "Replay it into mmcp once the server is back.\n",
        "\n",
        "## What each memory kind is worth\n",
        "\n",
        "A rule is absolute. It changes only after the operator's review and approval.  \n",
        "A feedback memory, under the `feedback/` prefix, is a rule candidate and a suggestion. It never outranks a rule.  \n",
        "Every other kind is a record: log, incident, scratch, snapshot, reference, feature, issue. A record is trusted as written.\n",
        "\n",
        "## When the rules are read\n",
        "\n",
        "### At the session start\n",
        "\n",
        "Call `mmcp__bootstrap_context` before any other tool call or file write.  \n",
        "Read every rule it points at, in full.\n",
        "\n",
        "### After a context compaction\n",
        "\n",
        "Read the rules again.  \n",
        "A compaction invalidates the rules only. The rest of its summary stays trusted.\n",
        "\n",
        "### Between those two moments\n",
        "\n",
        "Read the memory governing the subject at hand before working on it.\n",
        "<!-- mmcp:end v2 -->",
    );

    /// Number of lines of the approved block that end with a two-space hard break.
    const APPROVED_HARD_BREAK_LINES: usize = 9;

    #[test]
    fn user_claude_md_path_is_the_claude_directory_under_the_home() {
        match (mmcp_store::resolve_user_home(), user_claude_md_path()) {
            (Ok(home), Ok(path)) => {
                assert_eq!(path, home.join(".claude").join("CLAUDE.md"));
            }
            (Err(StoreError::HomeDirUnresolved), Err(StoreError::HomeDirUnresolved)) => {}
            (home, path) => panic!("home {home:?} and path {path:?} must agree"),
        }
    }

    #[test]
    fn render_block_is_the_approved_v2_block() {
        assert_eq!(render_block(), APPROVED_V2_BLOCK);
    }

    #[test]
    fn render_block_hard_breaks_only_sentences_followed_in_their_paragraph() {
        let hard_breaks = APPROVED_V2_BLOCK
            .lines()
            .filter(|line| line.ends_with("  "))
            .count();
        assert_eq!(hard_breaks, APPROVED_HARD_BREAK_LINES);
        assert!(
            APPROVED_V2_BLOCK
                .lines()
                .filter(|line| line.starts_with("<!--"))
                .all(|line| !line.ends_with(' ')),
            "fence markers and comment lines take no hard break"
        );
    }

    #[test]
    fn stub_is_the_title_then_the_block_alone() {
        assert_eq!(
            stub_contents(),
            format!("# CLAUDE.md\n\n{APPROVED_V2_BLOCK}\n")
        );
    }

    #[test]
    fn override_and_append_write_byte_identical_fenced_regions() {
        let stub = stub_contents();
        let appended = splice_block("# Hand-written\n\nIntro.\n").unwrap();
        let stub_fence = scan_fence(&stub).unwrap().expect("stub carries a fence");
        let appended_fence = scan_fence(&appended)
            .unwrap()
            .expect("append carries a fence");
        assert_eq!(stub_fence.region, appended_fence.region);
        assert_eq!(appended_fence.region, APPROVED_V2_BLOCK);
    }

    #[test]
    fn splice_block_appends_when_no_fence_present() {
        let input = "# Hello\n\nSome intro.\n";
        let out = splice_block(input).unwrap();
        assert_eq!(out, format!("{input}\n{APPROVED_V2_BLOCK}\n"));
    }

    #[test]
    fn splice_block_is_idempotent() {
        let once = splice_block("# Hello\n\nSome intro.\n").unwrap();
        let twice = splice_block(&once).unwrap();
        assert_eq!(once, twice);
    }

    #[test]
    fn splice_block_replaces_existing_current_version_fence() {
        let input = format!(
            "# Hello\n\nIntro.\n\n{begin}\nstale body\n{end}\n\nTrailer.\n",
            begin = begin_marker(),
            end = end_marker()
        );
        let out = splice_block(&input).unwrap();
        assert_eq!(
            out,
            format!("# Hello\n\nIntro.\n\n{APPROVED_V2_BLOCK}\n\nTrailer.\n")
        );
    }

    #[test]
    fn splice_block_upgrades_older_fence_version_in_place() {
        let input = "# X\n\n<!-- mmcp:begin v0 -->\nold\n<!-- mmcp:end v0 -->\n";
        let out = splice_block(input).unwrap();
        assert_eq!(out, format!("# X\n\n{APPROVED_V2_BLOCK}\n"));
    }

    #[test]
    fn splice_block_upgrades_a_v1_fence_preserving_every_outside_byte() {
        let prefix = "# CLAUDE.md\n\nOperator notes kept verbatim.  \r\nSecond line.\n\n";
        let v1_fence = concat!(
            "<!-- mmcp:begin v1 -->\n",
            "<!-- DO NOT EDIT inside this block. -->\n",
            "\n",
            "## MANDATORY: Re-read rules at every checkpoint\n",
            "\n",
            "Call `mmcp__bootstrap_context` before and after every commit cycle.\n",
            "<!-- mmcp:end v1 -->",
        );
        let suffix = "\n\n## Local section\n\nTrailing content, no final newline";
        let input = format!("{prefix}{v1_fence}{suffix}");

        let out = splice_block(&input).unwrap();

        assert_eq!(out, format!("{prefix}{APPROVED_V2_BLOCK}{suffix}"));
        assert!(out.starts_with(prefix));
        assert!(out.ends_with(suffix));
    }

    #[test]
    fn scan_fence_reports_the_begin_marker_version() {
        let input = "a\n<!-- mmcp:begin v1 -->\nbody\n<!-- mmcp:end v1 -->\nb\n";
        let fence = scan_fence(input).unwrap().expect("fence present");
        assert_eq!(fence.version, "v1");
        assert_eq!(
            fence.region,
            "<!-- mmcp:begin v1 -->\nbody\n<!-- mmcp:end v1 -->"
        );
        assert_eq!(&input[fence.range.clone()], fence.region);
        assert!(!fence.is_current());
    }

    #[test]
    fn scan_fence_flags_a_current_version_fence_with_a_non_canonical_body() {
        let input = format!("{}\nedited by hand\n{}\n", begin_marker(), end_marker());
        let fence = scan_fence(&input).unwrap().expect("fence present");
        assert_eq!(fence.version, BLOCK_VERSION);
        assert!(!fence.is_current());
    }

    #[test]
    fn scan_fence_accepts_the_current_render() {
        let stub = stub_contents();
        let fence = scan_fence(&stub).unwrap().expect("fence present");
        assert!(fence.is_current());
    }

    #[test]
    fn scan_fence_is_none_without_any_marker() {
        assert_eq!(scan_fence("# Plain\n\nNo fence here.\n").unwrap(), None);
    }

    #[test]
    fn scan_fence_stops_at_the_first_end_marker_after_the_begin_marker() {
        let first = "<!-- mmcp:begin v1 -->\none\n<!-- mmcp:end v1 -->";
        let between = "\nuser content between two fences\n";
        let second = "<!-- mmcp:begin v1 -->\ntwo\n<!-- mmcp:end v1 -->";
        let input = format!("{first}{between}{second}");
        let out = splice_block(&input).unwrap();
        assert_eq!(out, format!("{APPROVED_V2_BLOCK}{between}{second}"));
    }

    #[test]
    fn split_sections_preserves_h2_headers() {
        let input = "## Alpha\n\nMUST do alpha.\n\n## Beta\n\nBeta note.\n";
        let sections = split_sections(input);
        assert_eq!(sections.len(), 2);
        assert_eq!(sections[0].title, "Alpha");
        assert_eq!(sections[0].kind, MemoryKind::Rule);
        assert!(sections[0].mandatory);
        assert_eq!(sections[1].title, "Beta");
        assert_eq!(sections[1].kind, MemoryKind::Reference);
        assert!(!sections[1].mandatory);
    }

    #[test]
    fn split_sections_derives_unique_slugs() {
        let input = "## Rules\n\nX\n\n## Rules\n\nY\n";
        let sections = split_sections(input);
        assert_eq!(sections[0].slug, "imported-rules");
        assert_eq!(sections[1].slug, "imported-rules-2");
    }

    #[test]
    fn split_sections_captures_h3_subheaders_as_tags() {
        let input = "## Git\n\n### Commits\n\nMUST commit with signed keys.\n";
        let sections = split_sections(input);
        assert!(sections[0].tags.iter().any(|t| t == "commits"));
    }

    // Fence-marker identity

    #[test]
    fn markers_include_block_version_and_polarity() {
        // Exact strings the splicer, the bootstrap_context notes and
        // every existing CLAUDE.md depend on.
        assert_eq!(BLOCK_VERSION, "v2");
        assert_eq!(begin_marker(), "<!-- mmcp:begin v2 -->");
        assert_eq!(end_marker(), "<!-- mmcp:end v2 -->");
    }

    // Action resolution per explicit flag

    #[test]
    fn resolve_action_recognizes_convert_flag() {
        let mut args = default_args();
        args.convert = true;
        assert_eq!(resolve_action(&args, false).unwrap(), Action::Convert);
    }

    #[test]
    fn resolve_action_recognizes_append_flag() {
        let mut args = default_args();
        args.append = true;
        assert_eq!(resolve_action(&args, false).unwrap(), Action::Append);
    }

    #[test]
    fn resolve_action_rejects_override_plus_append() {
        let mut args = default_args();
        args.r#override = true;
        args.append = true;
        assert!(resolve_action(&args, false).is_err());
    }

    // splice_block boundary conditions

    #[test]
    fn splice_block_refuses_when_only_begin_marker_is_present() {
        // A run that crashed mid-write can leave a `begin` marker with
        // no matching `end`: the splicer refuses rather than guess at
        // the boundary.
        let input = format!("{begin}\npartial body\n", begin = begin_marker());
        let err = splice_block(&input).unwrap_err();
        assert_eq!(err, PartialFenceError::LoneBeginMarker);
        assert_eq!(
            err.to_string(),
            "CLAUDE.md carries a partial mmcp fence: a begin marker with no end marker after it; restore the end marker or remove the begin marker"
        );
    }

    #[test]
    fn splice_block_refuses_when_only_end_marker_is_present() {
        let input = format!("intro\n{end}\n", end = end_marker());
        let err = splice_block(&input).unwrap_err();
        assert_eq!(err, PartialFenceError::LoneEndMarker);
        assert_eq!(
            err.to_string(),
            "CLAUDE.md carries a partial mmcp fence: an end marker with no begin marker before it; restore the begin marker or remove the end marker"
        );
    }

    #[test]
    fn splice_block_refuses_an_end_marker_placed_before_the_begin_marker() {
        let input = "<!-- mmcp:end v1 -->\nswapped\n<!-- mmcp:begin v1 -->\n";
        let err = splice_block(input).unwrap_err();
        assert_eq!(err, PartialFenceError::LoneBeginMarker);
    }

    // slugify: pure-function coverage

    #[test]
    fn slugify_empty_input_falls_back_to_section_prefix() {
        assert_eq!(slugify(""), "imported-section");
        // All-punctuation likewise collapses to the fallback.
        assert_eq!(slugify("!!!"), "imported-section");
    }

    #[test]
    fn slugify_collapses_punctuation_into_single_hyphens() {
        // Multi-punct runs must not leave consecutive dashes.
        let slug = slugify("hello  world!!foo");
        assert_eq!(slug, "imported-hello-world-foo");
        // Trailing punctuation must not leave a trailing dash.
        assert_eq!(slugify("trailing!!"), "imported-trailing");
    }

    #[test]
    fn slugify_lowercases_and_strips_unicode() {
        // Non-ASCII alphanumerics drop through the fallback path;
        // ASCII letters get lowercased.
        assert_eq!(slugify("MixedCase"), "imported-mixedcase");
    }

    // uniquify: collision resolution

    #[test]
    fn uniquify_appends_sequential_suffix_for_repeated_collisions() {
        let mut seen = std::collections::HashSet::new();
        assert_eq!(uniquify("rules", &mut seen), "rules");
        assert_eq!(uniquify("rules", &mut seen), "rules-2");
        assert_eq!(uniquify("rules", &mut seen), "rules-3");
        assert_eq!(uniquify("rules", &mut seen), "rules-4");
        // Suffix increments monotonically: any mutation of `n += 1`
        // (for example `*=` or `-=`) breaks this chain.
    }

    #[test]
    fn uniquify_leaves_first_hit_untouched_and_only_disambiguates_later() {
        let mut seen = std::collections::HashSet::new();
        // Pre-seed the set so the first call sees a collision.
        seen.insert("rules".to_string());
        assert_eq!(uniquify("rules", &mut seen), "rules-2");
    }

    // first_paragraph: pure-function coverage

    #[test]
    fn first_paragraph_returns_first_non_empty_paragraph() {
        let body = "line one\nline two\n\nsecond paragraph\n";
        assert_eq!(first_paragraph(body), "line one line two");
    }

    #[test]
    fn first_paragraph_skips_leading_blank_lines() {
        let body = "\n\n\nfirst real line\nmore\n\nnext paragraph\n";
        assert_eq!(first_paragraph(body), "first real line more");
    }

    #[test]
    fn first_paragraph_falls_back_when_body_is_empty() {
        assert_eq!(first_paragraph(""), "Imported from CLAUDE.md");
        assert_eq!(first_paragraph("   \n\n   \n"), "Imported from CLAUDE.md");
    }

    #[test]
    fn first_paragraph_truncates_long_paragraphs_to_160_chars() {
        // 200 identical chars → must come back with the `...`
        // suffix and total length of 160. The `> 160` comparison in
        // the implementation is load-bearing; mutating it to `==`
        // or `<` would either never truncate or truncate short
        // strings.
        let long = "a".repeat(200);
        let out = first_paragraph(&long);
        assert_eq!(out.len(), 160);
        assert!(out.ends_with("..."));
    }

    #[test]
    fn first_paragraph_does_not_truncate_under_the_threshold() {
        // A 160-char input is the boundary, not greater, so no
        // truncation. Catches the `>` vs `>=` mutation.
        let just_at = "a".repeat(160);
        let out = first_paragraph(&just_at);
        assert_eq!(out.len(), 160);
        assert!(!out.ends_with("..."));
    }

    /// `convert` writes each section through the store's write path.
    /// A section whose body exceeds the write-time inline-result ceiling is refused, never committed lossily.
    #[tokio::test]
    async fn convert_refuses_a_section_over_the_result_ceiling() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let home = MmcpHome::from_root(tmp.path().join("home"));
        let (backend, groups) = home.init_backend().await.expect("init backend");

        let group_id = mmcp_core::id::GroupId::new();
        let owner = mmcp_core::id::UserId::new();
        let manifest =
            mmcp_core::manifest::GroupManifest::new_user_owned(group_id, "convert-target", owner);
        backend
            .create_group_repo(&manifest)
            .await
            .expect("create group repo");
        groups.refresh().await.expect("refresh");

        let project_root = tmp.path().join("project");
        std::fs::create_dir_all(&project_root).expect("mkdir project root");
        std::fs::write(
            project_root.join(PROJECT_MANIFEST),
            format!("project_uuid = \"{}\"\n", group_id.as_uuid()),
        )
        .expect("write .mmcp.toml");

        let author = home.resolve_author();
        let claude_md = format!("## Oversized\n\n{}\n", "a".repeat(60_000));
        let path = tmp.path().join("CLAUDE.md");

        let err = convert_and_write(&claude_md, &path, &project_root, &home, &author)
            .await
            .expect_err("a section over the ceiling must be refused");
        assert!(
            format!("{err:#}").contains("inline-result ceiling"),
            "expected a ceiling error, got: {err:#}"
        );
    }
}
