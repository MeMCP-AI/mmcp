//! CLI entry points for `mmcp check` and `mmcp diagnose`.
//!
//! The health / diagnostic analysis itself lives in
//! `mmcp_store::diagnostics` so MCP tool handlers, GUI widgets,
//! and third-party consumers can run the same checks without
//! pulling in the CLI's stdout formatting and exit-code mapping.
//! This module keeps only the three binary-specific bits: the
//! `run_check` / `run_diagnose` entries (walk groups, assemble
//! reports, map errors to `anyhow::Error`) and the `print_reports`
//! formatter. The formatter renders each group's summary
//! line then hands the collected findings to `render_notes_tail` so
//! the CLI tail mirrors the MCP `notes` channel verbatim.

use anyhow::Result;

use mmcp_proto::{Note, NoteLevel};
use mmcp_store::diagnostics::{
    DiagReport, GroupReport, diagnose_all, diagnose_group, health_check_all, health_check_group,
};
use mmcp_store::home::MmcpHome;
use mmcp_store::memory::resolve_group;

use crate::notes::{findings_to_notes, render_notes_tail};

// ── CLI entry points ────────────────────────────────────────

/// `mmcp check` - quick surface health check.
pub async fn run_check(group: Option<String>) -> Result<()> {
    let home = MmcpHome::discover()?;
    let (backend, groups) = home.init_backend().await?;

    let reports = if let Some(id) = group {
        let entry = resolve_group(&groups, &id)
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        vec![health_check_group(&backend, &entry).await]
    } else {
        health_check_all(&backend, &groups).await
    };

    let notes = print_reports(&reports, &[]);
    if notes.iter().any(|n| n.level == NoteLevel::Error) {
        std::process::exit(1);
    }
    // Any non-error note still counts as a finding on the
    // surface-check gate. `mmcp check` is the quick
    // pass-or-fail, so warnings flip exit code 1 too (matches
    // the old total-issues behaviour).
    if !notes.is_empty() {
        std::process::exit(1);
    }
    Ok(())
}

/// `mmcp diagnose` - deep diagnostic analysis.
pub async fn run_diagnose(group: Option<String>) -> Result<()> {
    let home = MmcpHome::discover()?;
    let (backend, groups) = home.init_backend().await?;

    let diag = if let Some(id) = group {
        let entry = resolve_group(&groups, &id)
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        DiagReport {
            project_findings: Vec::new(),
            groups: vec![diagnose_group(&backend, &entry).await],
        }
    } else {
        diagnose_all(&backend, &groups).await
    };

    let notes = print_reports(&diag.groups, &diag.project_findings);
    if notes.iter().any(|n| n.level == NoteLevel::Error) {
        std::process::exit(1);
    }
    Ok(())
}

/// Print the per-group structural summary then render every finding
/// on the notes tail. Returns the assembled notes so the
/// caller can branch on severity for its exit code. `project_findings`
/// carries the project/user-level diagnostics that belong to the
/// whole mirror rather than any one group (sync config, author
/// identity, CLAUDE.md); an empty slice is fine.
fn print_reports(
    reports: &[GroupReport],
    project_findings: &[mmcp_store::diagnostics::Finding],
) -> Vec<Note> {
    let mut notes: Vec<Note> = findings_to_notes(project_findings);
    for report in reports {
        notes.extend(findings_to_notes(&report.findings));
        println!(
            "{} ({}): {} memories, manifest_ok={}",
            report.slug, report.group_id, report.memory_count, report.manifest_ok
        );
    }

    let total_notes = notes.len();
    if total_notes == 0 {
        println!("\nall healthy");
    } else {
        println!("\n{total_notes} note(s) found");
    }
    render_notes_tail(&notes);
    notes
}
