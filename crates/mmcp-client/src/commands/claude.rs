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
use mmcp_git::{CommitSpec, GitBackend};

use mmcp_store::config::{self, PROJECT_MANIFEST};
use mmcp_store::home::{MmcpHome, ResolvedAuthor};

// ── Public CLI entry point ───────────────────────────────────────────

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
    #[arg(long, default_value = "CLAUDE.md")]
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

// ── Shared types ─────────────────────────────────────────────────────

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

// ── Constants the MCP side also reaches for ──────────────────────────

/// Version of the mmcp-managed block fence. Must match the value
/// `bootstrap_context` uses to flag stale fences.
pub const BLOCK_VERSION: &str = "v1";

pub fn begin_marker() -> String {
    format!("<!-- mmcp:begin {BLOCK_VERSION} -->")
}

pub fn end_marker() -> String {
    format!("<!-- mmcp:end {BLOCK_VERSION} -->")
}

// ── Action resolution ────────────────────────────────────────────────

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
            ActionChoice::Override => "override — replace CLAUDE.md with a fresh mmcp stub",
            ActionChoice::Append => "append — insert/replace the mmcp block inside CLAUDE.md",
            ActionChoice::Convert => "convert — split CLAUDE.md into typed memories, then stub",
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
    match Select::new("mmcp init claude — pick an action:", options).prompt() {
        Ok(ActionChoice::Override) => Ok(Action::Override),
        Ok(ActionChoice::Append) => Ok(Action::Append),
        Ok(ActionChoice::Convert) => Ok(Action::Convert),
        Ok(ActionChoice::Cancel)
        | Err(InquireError::OperationCanceled)
        | Err(InquireError::OperationInterrupted) => bail!("cancelled"),
        Err(e) => Err(anyhow!(e).context("reading action choice")),
    }
}

// ── File state detection ─────────────────────────────────────────────

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

// ── Safety prompts ───────────────────────────────────────────────────

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

// ── Execution ────────────────────────────────────────────────────────

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
            "cannot {} a missing CLAUDE.md — run with --override to write the stub first",
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
            let new = splice_block(&existing)?;
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

// ── Stub content (override mode) ─────────────────────────────────────

/// The generated CLAUDE.md body. Short and insistent: it points at
/// mmcp and lists the hard checkpoints where the AI MUST re-read
/// rules. Slugs, flags, and project-stack matrices live in the tool,
/// not here.
pub fn stub_contents() -> String {
    let begin = begin_marker();
    let end = end_marker();
    format!(
        "# CLAUDE.md

This file is managed by mmcp. Everything between the fence below is
regenerated on every `mmcp init claude` run: do not edit inside the
fence. Content outside the fence is preserved verbatim.

{begin}
<!-- DO NOT EDIT inside this block. Regenerated by `mmcp init claude`. Edit memories via `mmcp__write_memory`. -->

## CRITICAL: mmcp is mandatory, NO EXCEPTIONS

mmcp is this project's single source of truth for every coding rule,
convention, and project note. **If mmcp is unreachable, STOP
IMMEDIATELY.** Do not write code, do not attempt workarounds, do not
proceed on any task. Inform the user and halt until mmcp is restored.

## MANDATORY: Re-read rules at every checkpoint

The mandatory / project / language filter policy lives in the mmcp
tool, not here. **Do not hardcode slugs or YES/NO flags in this file.**
Call `mmcp__bootstrap_context` and read every returned memory body at
**EACH** of the following checkpoints. These are NOT suggestions.

- **Session start**: before any tool call or file write.
- **After ANY context compaction**: compaction summaries are NOT
  authoritative; the memories are. NEVER trust a compaction report.
- **Before starting a new phase or task**: refresh context every
  time, not once per session.
- **Before a commit cycle**: git conventions and coding rules may
  have shipped updates mid-session.
- **After a commit cycle**: re-align against the latest rules before
  picking up the next step.
- **Any time a rule is corrected, added, or discussed**: the memory
  may have been updated; re-read it.

Skipping any of these leaves your context stale against rules that
may have changed. Every checkpoint call is cheap: one tool
invocation, all memory bodies inline. Run it.

## Authoring rules

Use `mmcp__write_memory` to add or update rules. **Never** add project
rules directly to this file: CLAUDE.md's only job is to point at
mmcp. If this block says something is missing or stale, call
`mmcp__init_claude` explicitly; never hand-edit the managed region.
{end}
"
    )
}

// ── Append mode (fenced mmcp block) ──────────────────────────────────

/// Insert or replace the mmcp-managed block inside `existing`.
pub fn splice_block(existing: &str) -> Result<String> {
    let begin = begin_marker();
    let end = end_marker();
    let block = mmcp_block();

    // Case 1: current-version fence present on both ends → replace body.
    if let (Some(b), Some(e)) = (existing.find(&begin), existing.rfind(&end))
        && b < e
    {
        let mut out = String::with_capacity(existing.len());
        out.push_str(&existing[..b]);
        out.push_str(&block);
        out.push_str(&existing[e + end.len()..]);
        return Ok(out);
    }

    // Case 2: any older fence present → refuse; user must upgrade intentionally.
    if existing.contains("<!-- mmcp:begin ") || existing.contains("<!-- mmcp:end ") {
        return Err(anyhow!(
            "CLAUDE.md carries an mmcp fence of a different version; run with --override to regenerate the file fresh, or remove the old fence manually"
        ));
    }

    // Case 3: no fence → append the block at end of file with a separator.
    let mut out = existing.to_string();
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out.push('\n');
    out.push_str(&block);
    Ok(out)
}

/// Body of the managed block (between the fences, inclusive).
fn mmcp_block() -> String {
    let begin = begin_marker();
    let end = end_marker();
    format!(
        "{begin}
<!-- DO NOT EDIT inside this block. Regenerated by `mmcp init claude`. Edit memories via `mmcp__write_memory`. -->

## MANDATORY: Re-read rules at every checkpoint

Call `mmcp__bootstrap_context` at session start, after ANY compaction,
before and after every commit cycle, and at every phase boundary. The
tool owns the curated memory list; do not hardcode slugs.

Never trust a compaction summary. Never hand-edit this block.
{end}
"
    )
}

// ── Convert mode ─────────────────────────────────────────────────────

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
        let commit_id = backend
            .write_commit(
                &entry.handle,
                CommitSpec::mmcp_commit(
                    format!("convert CLAUDE.md section: {}", section.title),
                    vec![(
                        mmcp_core::conventions::memory_path(
                            &section.slug,
                            mmcp_core::id::MemoryId::new(),
                        ),
                        Some(rendered.into_bytes()),
                    )],
                    &author.name,
                    &author.email,
                ),
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

// ── Reporting ────────────────────────────────────────────────────────

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

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

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

    #[test]
    fn splice_block_appends_when_no_fence_present() {
        let input = "# Hello\n\nSome intro.\n";
        let out = splice_block(input).unwrap();
        assert!(out.contains(&begin_marker()));
        assert!(out.contains(&end_marker()));
        assert!(out.starts_with("# Hello"));
    }

    #[test]
    fn splice_block_replaces_existing_current_version_fence() {
        let input = format!(
            "# Hello\n\nIntro.\n\n{begin}\nstale body\n{end}\n\nTrailer.\n",
            begin = begin_marker(),
            end = end_marker()
        );
        let out = splice_block(&input).unwrap();
        assert!(!out.contains("stale body"));
        assert!(out.contains("Trailer."));
        assert!(out.contains("MANDATORY: Re-read rules at every checkpoint"));
    }

    #[test]
    fn splice_block_refuses_older_fence_version() {
        let input = "# X\n\n<!-- mmcp:begin v0 -->\nold\n<!-- mmcp:end v0 -->\n";
        let err = splice_block(input).unwrap_err();
        assert!(err.to_string().contains("different version"));
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

    #[test]
    fn stub_contains_fenced_block() {
        let stub = stub_contents();
        assert!(stub.contains(&begin_marker()));
        assert!(stub.contains(&end_marker()));
        assert!(stub.contains("CRITICAL: mmcp is mandatory"));
        assert!(stub.contains("bootstrap_context"));
    }

    // ── Fence-marker identity ───────────────────────────────────

    #[test]
    fn markers_include_block_version_and_polarity() {
        // `begin_marker` and `end_marker` must return the exact
        // strings the splicer, the bootstrap_context diagnostics,
        // and future version-upgraders depend on. A mutation that
        // replaced the body with any constant string would escape
        // unless we assert on the full contents directly.
        assert_eq!(
            begin_marker(),
            format!("<!-- mmcp:begin {BLOCK_VERSION} -->")
        );
        assert_eq!(end_marker(), format!("<!-- mmcp:end {BLOCK_VERSION} -->"));
        assert_ne!(begin_marker(), end_marker());
        assert!(begin_marker().contains("mmcp:begin"));
        assert!(end_marker().contains("mmcp:end"));
    }

    // ── Action resolution per explicit flag ─────────────────────

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

    // ── splice_block boundary conditions ────────────────────────

    #[test]
    fn splice_block_refuses_when_only_begin_marker_is_present() {
        // A previous run that crashed mid-write can leave a file
        // with a `begin` marker and no matching `end`. The splicer
        // must refuse rather than guess at the boundary: mutation
        // testing flagged this branch (the `||` between the two
        // "older fence" checks) as escaping.
        let input = format!("{begin}\npartial body\n", begin = begin_marker());
        let err = splice_block(&input).unwrap_err();
        assert!(err.to_string().contains("different version"));
    }

    #[test]
    fn splice_block_refuses_when_only_end_marker_is_present() {
        let input = format!("intro\n{end}\n", end = end_marker());
        let err = splice_block(&input).unwrap_err();
        assert!(err.to_string().contains("different version"));
    }

    // ── slugify: pure-function coverage ─────────────────────────

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

    // ── uniquify: collision resolution ──────────────────────────

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

    // ── first_paragraph: pure-function coverage ─────────────────

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
}
