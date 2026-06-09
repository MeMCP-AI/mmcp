//! CLI entry point for `mmcp import` + `protected_confirm` TUI
//! helper. The underlying memory CRUD and archive primitives live in
//! `mmcp_store`; this file adds the bits that cannot live in the store
//! because they depend on `inquire` / clap: the protected-group
//! confirm prompt and the clap `run` dispatch.
//!
//! `mmcp import` is a single ingest verb with three input shapes:
//! a loose `--file`, a `--dir` of loose files, or a portable
//! `--archive` (tar, gzip auto-detected). Loose inputs target one
//! `--group`; archives carry their own groups and optionally remap
//! into one via `--into`.

use std::io::IsTerminal;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use inquire::Confirm;
use mmcp_core::id::GroupId;
use mmcp_store::groups::{GroupEntry, GroupIndex};
use mmcp_store::home::{MmcpHome, ResolvedAuthor};
use mmcp_store::import_adoc::{convert_adoc_to_markdown, is_adoc_filename};
use mmcp_store::memory::{
    ImportError, SynthFrontmatter, import_memory, parse_kind, resolve_group, slugify_filename,
};
use mmcp_store::{ArchiveManifest, ImportArchiveOptions, import_archive, inspect_archive};

/// Arguments for `mmcp import`. The three input shapes (`--file`,
/// `--dir`, `--archive`) are mutually exclusive; loose-only and
/// archive-only knobs are conflict-gated so an invalid combination
/// fails at parse time.
#[derive(clap::Args)]
pub struct ImportArgs {
    /// Target group for loose `--file` / `--dir` import (UUID or
    /// slug). Archives carry their own groups; use `--into` to remap.
    #[arg(long, conflicts_with = "archive")]
    pub group: Option<String>,

    /// Import a single `.md` / `.adoc` file into `--group`.
    #[arg(long, conflicts_with_all = ["dir", "archive"])]
    pub file: Option<PathBuf>,

    /// Import every `.md` / `.adoc` file in this directory.
    #[arg(long, conflicts_with_all = ["file", "archive"])]
    pub dir: Option<PathBuf>,

    /// Import a portable mmcp archive (tar; gzip auto-detected),
    /// recreating its groups by uuid.
    #[arg(long, conflicts_with_all = ["file", "dir"])]
    pub archive: Option<PathBuf>,

    /// Override the slug (loose `--file` import only).
    #[arg(long, conflicts_with_all = ["dir", "archive"], requires = "file")]
    pub slug: Option<String>,

    /// Memory name (loose import when the file has no +++ frontmatter).
    #[arg(long, conflicts_with = "archive")]
    pub name: Option<String>,

    /// Memory description (loose import without +++ frontmatter).
    #[arg(long, conflicts_with = "archive")]
    pub description: Option<String>,

    /// Memory kind (loose import without +++ frontmatter).
    #[arg(long, conflicts_with = "archive")]
    pub kind: Option<String>,

    /// Archive import: remap every memory into this existing group
    /// (UUID or slug) instead of recreating the archived groups.
    #[arg(long, conflicts_with_all = ["file", "dir"], requires = "archive")]
    pub into: Option<String>,

    /// Archive import: mint fresh UUIDs for every imported memory
    /// (fork / copy) instead of preserving the archived identities.
    #[arg(long, default_value_t = false, conflicts_with_all = ["file", "dir"], requires = "archive")]
    pub new_ids: bool,

    /// Archive import: import only these archived groups (UUID or
    /// slug, repeatable). Empty imports every group in the archive.
    #[arg(long = "only-group", conflicts_with_all = ["file", "dir"], requires = "archive")]
    pub only_group: Vec<String>,

    /// Archive import: import only memories whose slug is in this set
    /// (repeatable). Empty imports every memory in the chosen groups.
    #[arg(long = "only-memory", conflicts_with_all = ["file", "dir"], requires = "archive")]
    pub only_memory: Vec<String>,

    /// Replace an existing memory instead of erroring (loose) or
    /// overwrite colliding memories (archive). Default is strict.
    #[arg(long, default_value_t = false)]
    pub r#override: bool,

    /// Skip the protected-group confirmation prompt. Required on
    /// non-TTY stdin when a target group is protected.
    #[arg(long, default_value_t = false)]
    pub force: bool,
}

/// CLI-side parity of the MCP `ensure_not_protected` guard.
///
/// When the target group is protected, prompts the operator with
/// `inquire::Confirm` on a TTY (default: no). Non-TTY invocations
/// must pass `force = true` explicitly so scripted imports never
/// silently poke at protected groups. Unprotected groups are a
/// no-op — the operator's `mmcp import` intent is the confirmation.
pub fn protected_confirm(entry: &GroupEntry, force: bool) -> Result<()> {
    if !entry.manifest.protected {
        return Ok(());
    }
    eprintln!(
        "notice: group `{}` is marked protected; writes into it are audited.",
        entry.manifest.slug,
    );
    if force {
        return Ok(());
    }
    if !std::io::stdin().is_terminal() {
        bail!(
            "group `{}` is protected; pass --force on non-TTY invocations to confirm the write",
            entry.manifest.slug,
        );
    }
    let prompt = format!(
        "Writing into protected group `{}` — continue?",
        entry.manifest.slug,
    );
    let confirmed = Confirm::new(&prompt)
        .with_default(false)
        .prompt()
        .context("reading protected-group confirmation from TTY")?;
    if !confirmed {
        bail!("aborted: operator declined to write into protected group");
    }
    Ok(())
}

// ── CLI entry point ─────────────────────────────────────────────

/// CLI entry point for `mmcp import`. Dispatches on the input shape:
/// an `--archive` routes to the archive importer, otherwise a loose
/// `--file` / `--dir` import runs.
pub async fn run(args: ImportArgs) -> Result<()> {
    let mmcp_home = MmcpHome::discover()?;
    let (backend, group_index) = mmcp_home.init_backend().await?;
    let author = mmcp_home.resolve_author();

    if let Some(path) = args.archive.clone() {
        return run_archive(&backend, &group_index, &author, &args, &path).await;
    }
    run_loose(&backend, &group_index, &author, args).await
}

/// Loose `--file` / `--dir` import of markdown / AsciiDoc into one
/// group. `override_existing` mirrors the MCP `write_memory` tool's
/// `override` argument: `false` (default) is strict CREATE and errors
/// on a slug already on disk; `true` replaces the file in place.
async fn run_loose(
    backend: &mmcp_git::NativeBackend,
    group_index: &GroupIndex,
    author: &ResolvedAuthor,
    args: ImportArgs,
) -> Result<()> {
    let Some(group) = args.group.clone() else {
        bail!("--group is required for --file / --dir import");
    };

    let entry = resolve_group(group_index, &group)
        .await
        .with_context(|| format!("resolving group '{group}'"))?;

    protected_confirm(&entry, args.force)?;

    let synth = match (args.name, args.description, args.kind) {
        (Some(n), Some(d), Some(k)) => Some(SynthFrontmatter {
            name: n,
            description: d,
            kind: parse_kind(&k)?,
        }),
        (None, None, None) => None,
        _ => {
            bail!("--name, --description, and --kind must all be provided together or all omitted")
        }
    };

    if let Some(path) = args.file {
        let content = load_import_source(&path)?;
        let slug = args.slug.unwrap_or_else(|| {
            slugify_filename(
                path.file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("unnamed"),
            )
        });
        match import_memory(
            backend,
            &entry.handle,
            &slug,
            &content,
            synth,
            author,
            args.r#override,
        )
        .await
        {
            Ok(result) => {
                println!("imported {} (commit {})", result.slug, result.commit_id);
            }
            Err(ImportError::MemoryAlreadyExists { slug }) => {
                bail!(
                    "memory `{slug}` already exists in group `{group}`; pass --override to replace it, or use `mmcp__edit_memory` for partial updates"
                );
            }
            Err(err) => return Err(err.into()),
        }
    } else if let Some(dir_path) = args.dir {
        let mut count = 0;
        let mut entries: Vec<_> = std::fs::read_dir(&dir_path)
            .with_context(|| format!("reading directory {}", dir_path.display()))?
            .filter_map(|e| e.ok())
            .filter(|e| is_supported_import_extension(&e.path()))
            .collect();
        entries.sort_by_key(|e| e.file_name());

        for entry_file in entries {
            let path = entry_file.path();
            let slug = slugify_filename(
                path.file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("unnamed"),
            );
            let content = match load_import_source(&path) {
                Ok(c) => c,
                Err(err) => {
                    eprintln!("skipped {}: {err}", path.display());
                    continue;
                }
            };
            match import_memory(
                backend,
                &entry.handle,
                &slug,
                &content,
                synth.clone(),
                author,
                args.r#override,
            )
            .await
            {
                Ok(result) => {
                    println!("imported {} (commit {})", result.slug, result.commit_id);
                    count += 1;
                }
                Err(err) => {
                    eprintln!("skipped {}: {err}", path.display());
                }
            }
        }
        println!("{count} memories imported");
    } else {
        bail!("either --file, --dir, or --archive is required");
    }

    Ok(())
}

/// Archive import: read the artifact, confirm any protected target
/// groups, then replay it through the store importer and print a
/// per-group summary.
async fn run_archive(
    backend: &mmcp_git::NativeBackend,
    group_index: &GroupIndex,
    author: &ResolvedAuthor,
    args: &ImportArgs,
    path: &Path,
) -> Result<()> {
    let bytes =
        std::fs::read(path).with_context(|| format!("reading archive {}", path.display()))?;
    let manifest = inspect_archive(&bytes).context("reading archive table of contents")?;

    let into_group = match &args.into {
        Some(query) => {
            let entry = resolve_group(group_index, query)
                .await
                .with_context(|| format!("resolving --into group '{query}'"))?;
            Some(GroupId::from_uuid(entry.handle.group_id))
        }
        None => None,
    };

    // Confirm every existing protected group the import will write
    // into before any write happens; new groups are authorised by the
    // import intent itself.
    confirm_protected_targets(group_index, &manifest, into_group, args.force).await?;

    let options = ImportArchiveOptions {
        into_group,
        overwrite: args.r#override,
        new_ids: args.new_ids,
        allow_protected: true,
        select_groups: args.only_group.clone(),
        select_memory_slugs: args.only_memory.clone(),
    };
    let report = import_archive(backend, group_index, author, &bytes, &options).await?;

    for group in &report.groups {
        let action = if group.created_group {
            "created group"
        } else {
            "merged into group"
        };
        println!(
            "{action} {} ({}): {} created, {} overwritten, {} skipped, {} conflicts",
            group.slug,
            group.target_group_id,
            group.created,
            group.overwritten,
            group.skipped,
            group.conflicts.len(),
        );
        for conflict in &group.conflicts {
            eprintln!(
                "  conflict (pass --override to replace): {} ({})",
                conflict.slug, conflict.id,
            );
        }
    }
    Ok(())
}

/// Prompt for confirmation on every existing protected group an
/// archive import would write into. New groups (recreated from the
/// archive) carry no local protection to confirm.
async fn confirm_protected_targets(
    group_index: &GroupIndex,
    manifest: &ArchiveManifest,
    into_group: Option<GroupId>,
    force: bool,
) -> Result<()> {
    let targets = if let Some(group_id) = into_group {
        group_index.get(&group_id).await.into_iter().collect()
    } else {
        let mut out = Vec::new();
        for group_meta in &manifest.groups {
            if let Ok(entry) = resolve_group(group_index, &group_meta.group_id.to_string()).await {
                out.push(entry);
            }
        }
        out
    };

    for entry in &targets {
        if entry.manifest.protected {
            protected_confirm(entry, force)?;
        }
    }
    Ok(())
}

/// Read an import source file and return markdown-ready content.
///
/// Markdown sources pass through verbatim; `.adoc` / `.asciidoc`
/// sources get rendered to CommonMark via `acdc` so the on-disk
/// memory can land at `memories/<slug>/<uuid>.md` with the same
/// frontmatter semantics as a native markdown import.
fn load_import_source(path: &Path) -> Result<String> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("reading {}", path.display()))?;
    let filename = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    if is_adoc_filename(filename) {
        convert_adoc_to_markdown(&raw)
            .with_context(|| format!("converting AsciiDoc {}", path.display()))
    } else {
        Ok(raw)
    }
}

/// True when the path's extension matches one of the import formats
/// the pipeline knows how to normalise to markdown. Used by the
/// `--dir` filter so `.adoc` and `.asciidoc` files get picked up
/// alongside `.md` without the caller spelling them out.
fn is_supported_import_extension(path: &Path) -> bool {
    let Some(filename) = path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    path.extension().is_some_and(|ext| ext == "md") || is_adoc_filename(filename)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn load_import_source_passes_markdown_through_unchanged() {
        let tmp = TempDir::new().expect("tempdir");
        let path = tmp.path().join("note.md");
        std::fs::write(&path, "# Title\n\nBody paragraph.\n").unwrap();
        let out = load_import_source(&path).expect("load md");
        assert_eq!(out, "# Title\n\nBody paragraph.\n");
    }

    #[test]
    fn load_import_source_converts_adoc_to_markdown() {
        let tmp = TempDir::new().expect("tempdir");
        let path = tmp.path().join("rules.adoc");
        std::fs::write(&path, "= Heading\n\nParagraph text.\n").unwrap();
        let out = load_import_source(&path).expect("load adoc");
        assert!(
            out.contains("Heading"),
            "adoc heading must survive conversion; got: {out}"
        );
        assert!(
            out.contains("Paragraph text."),
            "adoc paragraph must survive conversion; got: {out}"
        );
        assert!(
            !out.starts_with("= "),
            "adoc-native heading syntax must be rewritten; got: {out}"
        );
    }

    #[test]
    fn is_supported_import_extension_accepts_md_and_adoc_variants() {
        assert!(is_supported_import_extension(Path::new("a.md")));
        assert!(is_supported_import_extension(Path::new("a.adoc")));
        assert!(is_supported_import_extension(Path::new("a.asciidoc")));
        assert!(is_supported_import_extension(Path::new("A.ADOC")));
        assert!(!is_supported_import_extension(Path::new("a.txt")));
        assert!(!is_supported_import_extension(Path::new("a")));
        assert!(!is_supported_import_extension(Path::new(".md")));
    }
}
