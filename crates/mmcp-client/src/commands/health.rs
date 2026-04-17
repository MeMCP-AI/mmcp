//! CLI entry points for `mmcp check` and `mmcp diagnose`.
//!
//! The actual health / diagnostic analysis now lives in
//! `mmcp_store::diagnostics` so MCP tool handlers, GUI widgets,
//! and third-party consumers can run the same checks without
//! pulling in the CLI's stdout formatting and exit-code mapping.
//! This module keeps only the three binary-specific bits: the
//! `run_check` / `run_diagnose` entries (walk groups, assemble
//! reports, map errors to `anyhow::Error`) and the `print_reports`
//! formatter.

use anyhow::Result;

use mmcp_store::diagnostics::{
    DiagReport, GroupReport, diagnose_all, diagnose_group, health_check_all, health_check_group,
};
use mmcp_store::home::MmcpHome;
use mmcp_store::memory::resolve_group;

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

    print_reports(&reports);
    let total: usize = reports.iter().map(|r| r.issues.len()).sum();
    if total > 0 {
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
            project_issues: Vec::new(),
            groups: vec![diagnose_group(&backend, &entry).await],
        }
    } else {
        diagnose_all(&backend, &groups).await
    };

    // Print project-level issues
    if !diag.project_issues.is_empty() {
        println!("project:");
        for issue in &diag.project_issues {
            println!("  [{}] {}", issue.severity, issue.message);
        }
    }

    print_reports(&diag.groups);
    let errors: usize = diag
        .groups
        .iter()
        .flat_map(|r| &r.issues)
        .chain(diag.project_issues.iter())
        .filter(|i| i.severity == "error")
        .count();
    if errors > 0 {
        std::process::exit(1);
    }
    Ok(())
}

fn print_reports(reports: &[GroupReport]) {
    for report in reports {
        if report.issues.is_empty() {
            println!(
                "{} ({}): {} memories, healthy",
                report.slug, report.group_id, report.memory_count
            );
        } else {
            let errors = report
                .issues
                .iter()
                .filter(|i| i.severity == "error")
                .count();
            let warnings = report
                .issues
                .iter()
                .filter(|i| i.severity == "warning")
                .count();
            let infos = report
                .issues
                .iter()
                .filter(|i| i.severity == "info")
                .count();
            println!(
                "{} ({}): {} memories, {} error(s), {} warning(s), {} info(s):",
                report.slug, report.group_id, report.memory_count, errors, warnings, infos
            );
            for issue in &report.issues {
                let target = issue.slug.as_deref().unwrap_or("manifest");
                println!("  [{}] {}: {}", issue.severity, target, issue.message);
            }
        }
    }

    let total_issues: usize = reports.iter().map(|r| r.issues.len()).sum();
    if total_issues == 0 {
        println!("\nall healthy");
    } else {
        println!("\n{total_issues} issue(s) found");
    }
}
