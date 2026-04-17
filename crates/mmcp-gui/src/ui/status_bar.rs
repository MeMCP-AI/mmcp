//! Bottom status bar: summary of the sync state plus anything
//! interesting about the current selection.
//!
//! Render order: left side shows a compact sync descriptor (idle /
//! syncing / last-ok / last-err); right side shows the selected
//! group's slug and memory count. The layout uses
//! `with_layout(RightToLeft)` to anchor the right-side summary to
//! the edge without pre-computing widths.

use eframe::egui;

use crate::state::AppState;
use crate::state::sync_reachability::SyncReachability;
use crate::state::sync_status::SyncStatus;

pub fn show(ui: &mut egui::Ui, state: &AppState) {
    egui::Panel::bottom("mmcp_gui_status_bar").show_inside(ui, |ui| {
        ui.horizontal(|ui| {
            render_reachability(ui, &state.reachability, &state.sync);
            ui.separator();
            render_sync(ui, &state.sync);

            if let Some(report) = &state.diag_report {
                ui.separator();
                render_diag_summary(ui, report);
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                render_selection_summary(ui, state);
            });
        });
    });
}

fn render_reachability(ui: &mut egui::Ui, reach: &SyncReachability, sync: &SyncStatus) {
    // If sync isn't configured, connectivity is moot.
    if matches!(sync, SyncStatus::NotConfigured) {
        return;
    }
    match reach {
        SyncReachability::Unknown => {
            ui.colored_label(egui::Color32::GRAY, "● probing…");
        }
        SyncReachability::Online => {
            ui.colored_label(egui::Color32::from_rgb(120, 200, 120), "● online");
        }
        SyncReachability::Offline { reason } => {
            ui.colored_label(egui::Color32::from_rgb(220, 120, 120), "● offline")
                .on_hover_text(reason);
        }
    }
}

fn render_diag_summary(ui: &mut egui::Ui, report: &mmcp_store::DiagReport) {
    let (errors, warnings, _infos) = crate::ui::diagnostics_panel::count_severities(report);
    let color = if errors > 0 {
        egui::Color32::from_rgb(220, 120, 120)
    } else if warnings > 0 {
        egui::Color32::from_rgb(220, 180, 80)
    } else {
        egui::Color32::from_rgb(120, 200, 120)
    };
    ui.colored_label(
        color,
        format!("diagnose: {errors} errors, {warnings} warnings"),
    );
}

fn render_sync(ui: &mut egui::Ui, status: &SyncStatus) {
    match status {
        SyncStatus::Unknown => {
            ui.label("sync: starting…");
        }
        SyncStatus::NotConfigured => {
            ui.colored_label(egui::Color32::GRAY, "sync: not configured");
        }
        SyncStatus::Idle { server_url } => {
            ui.label(format!("sync: idle ({server_url})"));
        }
        SyncStatus::Syncing { server_url, op } => {
            ui.spinner();
            ui.label(format!("sync: {}ing {server_url}", op.as_str()));
        }
        SyncStatus::LastOk {
            server_url,
            op,
            summary,
        } => {
            ui.colored_label(
                egui::Color32::from_rgb(120, 200, 120),
                format!("sync: {} ok ({server_url}) — {summary}", op.as_str()),
            );
        }
        SyncStatus::LastErr {
            server_url,
            op,
            message,
        } => {
            ui.colored_label(
                egui::Color32::from_rgb(220, 120, 120),
                format!("sync: {} failed ({server_url}) — {message}", op.as_str()),
            );
        }
    }
}

fn render_selection_summary(ui: &mut egui::Ui, state: &AppState) {
    let Some(group_id) = state.selection.group else {
        return;
    };
    let Some(entry) = state
        .groups
        .iter()
        .find(|g| g.manifest.group_id == group_id)
    else {
        return;
    };
    let slugs = state.memory_slugs.get(&group_id);
    let count = slugs.map(|s| s.len()).unwrap_or(0);
    ui.label(format!(
        "group: {} ({} memories)",
        entry.manifest.slug, count
    ));
}
