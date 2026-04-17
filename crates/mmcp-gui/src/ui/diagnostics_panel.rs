//! Floating window that renders a `DiagReport` with per-group
//! issue counts and a scrollable issue list.
//!
//! Shown when `state.diag_panel_open` is true; dismissed by the
//! window's native close button. The report itself is refreshed by
//! clicking the toolbar's "Diagnose" button, which fires a
//! `BackgroundTask::RunDiagnose`.

use eframe::egui;
use mmcp_store::{DiagReport, Issue};

use crate::state::AppState;

pub fn show(ctx: &egui::Context, state: &mut AppState) {
    if !state.diag_panel_open {
        return;
    }

    let mut open = state.diag_panel_open;
    egui::Window::new("Diagnostics")
        .open(&mut open)
        .default_size([560.0, 420.0])
        .resizable(true)
        .show(ctx, |ui| match &state.diag_report {
            None => {
                ui.spinner();
                ui.label("Running diagnose_all…");
            }
            Some(report) => render_report(ui, report),
        });
    state.diag_panel_open = open;
}

fn render_report(ui: &mut egui::Ui, report: &DiagReport) {
    let (errors, warnings, infos) = count_severities(report);
    ui.horizontal(|ui| {
        ui.colored_label(severity_color("error"), format!("{errors} errors"));
        ui.colored_label(severity_color("warn"), format!("{warnings} warnings"));
        ui.colored_label(severity_color("info"), format!("{infos} infos"));
    });
    ui.separator();

    egui::ScrollArea::vertical().show(ui, |ui| {
        if !report.project_issues.is_empty() {
            ui.heading("Project");
            for issue in &report.project_issues {
                render_issue(ui, issue);
            }
            ui.add_space(8.0);
        }

        for group in &report.groups {
            ui.heading(&group.slug);
            ui.label(format!(
                "manifest: {} • {} memories • {} issues",
                if group.manifest_ok { "ok" } else { "BROKEN" },
                group.memory_count,
                group.issues.len()
            ));
            for issue in &group.issues {
                render_issue(ui, issue);
            }
            ui.add_space(8.0);
        }
    });
}

fn render_issue(ui: &mut egui::Ui, issue: &Issue) {
    ui.horizontal_wrapped(|ui| {
        ui.colored_label(
            severity_color(issue.severity),
            format!("[{}]", issue.severity),
        );
        if let Some(slug) = &issue.slug {
            ui.label(format!("{slug}:"));
        }
        ui.label(&issue.message);
    });
}

fn severity_color(severity: &str) -> egui::Color32 {
    match severity {
        "error" => egui::Color32::from_rgb(220, 100, 100),
        "warn" => egui::Color32::from_rgb(220, 180, 80),
        _ => egui::Color32::from_rgb(140, 180, 220),
    }
}

pub fn count_severities(report: &DiagReport) -> (usize, usize, usize) {
    let mut errors = 0usize;
    let mut warnings = 0usize;
    let mut infos = 0usize;
    for issue in report
        .project_issues
        .iter()
        .chain(report.groups.iter().flat_map(|g| g.issues.iter()))
    {
        match issue.severity {
            "error" => errors += 1,
            "warn" => warnings += 1,
            _ => infos += 1,
        }
    }
    (errors, warnings, infos)
}
