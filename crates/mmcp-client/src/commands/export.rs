//! CLI entry point for `mmcp export`.
//!
//! Packages one or more groups into a portable archive on disk. The
//! batch counterpart to `mmcp import`: where import ingests, export
//! emits. Reads are non-destructive, so no protected-group prompt is
//! involved here.

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use mmcp_store::groups::GroupEntry;
use mmcp_store::home::MmcpHome;
use mmcp_store::{ExportOptions, export_archive, resolve_group, resolve_project_group};

/// CLI entry point for `mmcp export`.
///
/// Group selection precedence: `--all` exports every mirrored group;
/// otherwise each `--group` is resolved; with neither, the current
/// project's group is used (and a missing project is an error that
/// names the two explicit selectors).
pub async fn run(groups: Vec<String>, all: bool, output: PathBuf, gzip: bool) -> Result<()> {
    let mmcp_home = MmcpHome::discover()?;
    let (backend, group_index) = mmcp_home.init_backend().await?;

    let selected: Vec<GroupEntry> = if all {
        if !groups.is_empty() {
            bail!("--all cannot be combined with --group");
        }
        group_index.list().await
    } else if !groups.is_empty() {
        let mut out = Vec::with_capacity(groups.len());
        for group in &groups {
            out.push(
                resolve_group(&group_index, group)
                    .await
                    .with_context(|| format!("resolving group '{group}'"))?,
            );
        }
        out
    } else {
        let cwd = std::env::current_dir().context("resolving current directory")?;
        let (entry, _root) = resolve_project_group(&group_index, &cwd).await.context(
            "no --group or --all given and no project group found in the current directory",
        )?;
        vec![entry]
    };

    if selected.is_empty() {
        bail!("no groups to export");
    }

    let file = std::fs::File::create(&output)
        .with_context(|| format!("creating archive {}", output.display()))?;
    let manifest = export_archive(&backend, &selected, &ExportOptions { gzip }, file).await?;

    println!(
        "exported {} group(s), {} memories to {}",
        manifest.groups.len(),
        manifest.total_memory_count(),
        output.display(),
    );
    Ok(())
}
