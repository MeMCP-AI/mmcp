//! CLI entry point for `mmcp import` + `protected_confirm` TUI
//! helper. The underlying memory CRUD primitives now live in
//! `mmcp_store::memory`; this file re-exports them (for
//! back-compat with existing `crate::commands::import::…`
//! imports) and adds the two bits that cannot live in the store
//! because they depend on `inquire` / clap: the protected-group
//! confirm prompt and the clap `run` dispatch.

use std::io::IsTerminal;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use inquire::Confirm;
use mmcp_store::groups::GroupEntry;
use mmcp_store::home::MmcpHome;
use mmcp_store::memory::{
    ImportError, SynthFrontmatter, import_memory, parse_kind, resolve_group, slugify_filename,
};

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

/// CLI entry point for `mmcp import`.
///
/// `override_existing` mirrors the MCP `write_memory` tool's
/// `override` argument: `false` (default) is strict CREATE and
/// errors with a user-facing hint when the slug is already on
/// disk; `true` replaces the file in place. The collision error
/// names both `--override` and `mmcp__edit_memory` so operators
/// see the two escape hatches explicitly.
#[allow(clippy::too_many_arguments)]
pub async fn run(
    group: String,
    file: Option<PathBuf>,
    dir: Option<PathBuf>,
    slug_override: Option<String>,
    name: Option<String>,
    description: Option<String>,
    kind: Option<String>,
    override_existing: bool,
    force: bool,
) -> Result<()> {
    let mmcp_home = MmcpHome::discover()?;
    let (backend, group_index) = mmcp_home.init_backend().await?;
    let author = mmcp_home.resolve_author();

    let entry = resolve_group(&group_index, &group)
        .await
        .with_context(|| format!("resolving group '{group}'"))?;

    protected_confirm(&entry, force)?;

    let synth = match (name, description, kind) {
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

    if let Some(path) = file {
        let content = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        let slug = slug_override.unwrap_or_else(|| {
            slugify_filename(
                path.file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("unnamed"),
            )
        });
        match import_memory(
            &backend,
            &entry.handle,
            &slug,
            &content,
            synth,
            &author,
            override_existing,
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
    } else if let Some(dir_path) = dir {
        let mut count = 0;
        let mut entries: Vec<_> = std::fs::read_dir(&dir_path)
            .with_context(|| format!("reading directory {}", dir_path.display()))?
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().is_some_and(|ext| ext == "md"))
            .collect();
        entries.sort_by_key(|e| e.file_name());

        for entry_file in entries {
            let path = entry_file.path();
            let slug = slugify_filename(
                path.file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("unnamed"),
            );
            let content = std::fs::read_to_string(&path)
                .with_context(|| format!("reading {}", path.display()))?;
            match import_memory(
                &backend,
                &entry.handle,
                &slug,
                &content,
                synth.clone(),
                &author,
                override_existing,
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
        bail!("either --file or --dir is required");
    }

    Ok(())
}
