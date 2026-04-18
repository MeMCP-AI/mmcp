//! Graphical diagnostics panel.
//!
//! Replaces the earlier text-list rendering with a visual hierarchy:
//! severity tiles summarise the whole report, a filter bar lets the
//! user narrow to a single severity, and per-group cards
//! (collapsible, with a colored left accent encoding the worst
//! severity present) group the issues in context.
//!
//! Keeps parity with the CLI `mmcp diagnose` output because both
//! paths consume the same `mmcp_store::DiagReport` — only the
//! rendering differs. CLI is text because terminals are text; GUI
//! is graphical because users asked for graphical.

use std::collections::HashSet;

use eframe::egui;
use egui_phosphor::regular as icons;
use mmcp_store::{DiagReport, GroupReport, Issue};

use crate::state::AppState;

/// Which severities the list below the tiles shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SeverityFilter {
    #[default]
    All,
    Errors,
    Warnings,
    Infos,
}

impl SeverityFilter {
    fn matches(self, severity: &str) -> bool {
        match self {
            SeverityFilter::All => true,
            SeverityFilter::Errors => severity == "error",
            SeverityFilter::Warnings => severity == "warn",
            SeverityFilter::Infos => severity != "error" && severity != "warn",
        }
    }

    fn label(self) -> &'static str {
        match self {
            SeverityFilter::All => "All",
            SeverityFilter::Errors => "Errors",
            SeverityFilter::Warnings => "Warnings",
            SeverityFilter::Infos => "Infos",
        }
    }
}

/// Panel widget. Held by `MmcpGuiApp` so the filter + collapsed
/// state persist across frames and between Diagnose runs within the
/// same session.
#[derive(Default)]
pub struct DiagnosticsPanel {
    filter: SeverityFilter,
    /// Group slugs the user has collapsed. Default-expanded so a
    /// fresh Diagnose surfaces everything at once.
    collapsed: HashSet<String>,
    /// Special key for the optional "project" section, which isn't
    /// a real group but renders like one.
    project_collapsed: bool,
}

impl DiagnosticsPanel {
    pub fn show(&mut self, ctx: &egui::Context, state: &mut AppState) {
        if !state.diag_panel_open {
            return;
        }
        let mut open = state.diag_panel_open;
        egui::Window::new("Diagnostics")
            .open(&mut open)
            .default_size([680.0, 520.0])
            .resizable(true)
            .show(ctx, |ui| match &state.diag_report {
                None => {
                    ui.vertical_centered(|ui| {
                        ui.add_space(32.0);
                        ui.spinner();
                        ui.add_space(8.0);
                        ui.label("Running diagnose_all…");
                    });
                }
                Some(report) => self.render_report(ui, report),
            });
        state.diag_panel_open = open;
    }

    fn render_report(&mut self, ui: &mut egui::Ui, report: &DiagReport) {
        let (errors, warnings, infos) = count_severities(report);
        render_tiles(ui, errors, warnings, infos);
        ui.add_space(8.0);
        self.render_filter_bar(ui, errors, warnings, infos);
        ui.add_space(8.0);
        ui.separator();
        ui.add_space(6.0);

        egui::ScrollArea::vertical()
            .id_salt("mmcp_gui_diag_scroll")
            .show(ui, |ui| {
                self.render_project_card(ui, &report.project_issues);
                for group in &report.groups {
                    self.render_group_card(ui, group);
                }
            });
    }

    fn render_filter_bar(
        &mut self,
        ui: &mut egui::Ui,
        errors: usize,
        warnings: usize,
        infos: usize,
    ) {
        ui.horizontal_wrapped(|ui| {
            for (filter, count) in [
                (SeverityFilter::All, errors + warnings + infos),
                (SeverityFilter::Errors, errors),
                (SeverityFilter::Warnings, warnings),
                (SeverityFilter::Infos, infos),
            ] {
                let active = self.filter == filter;
                if filter_pill(ui, filter.label(), count, active).clicked() {
                    self.filter = filter;
                }
            }
        });
    }

    fn render_project_card(&mut self, ui: &mut egui::Ui, project_issues: &[Issue]) {
        let visible: Vec<&Issue> = project_issues
            .iter()
            .filter(|i| self.filter.matches(i.severity))
            .collect();
        if visible.is_empty() && project_issues.is_empty() {
            return;
        }

        let accent = group_accent(project_issues);
        group_card(ui, accent, |ui| {
            let (errors, warnings, infos) = count_issues(project_issues);
            ui.horizontal(|ui| {
                if disclosure_button(ui, !self.project_collapsed).clicked() {
                    self.project_collapsed = !self.project_collapsed;
                }
                ui.label(egui::RichText::new("Project").strong());
                ui.add_space(6.0);
                severity_mini_pills(ui, errors, warnings, infos);
            });
            if !self.project_collapsed {
                ui.add_space(4.0);
                if visible.is_empty() {
                    ui.label(
                        egui::RichText::new("(no issues at current filter)")
                            .small()
                            .weak(),
                    );
                } else {
                    for issue in visible {
                        render_issue_row(ui, issue);
                    }
                }
            }
        });
        ui.add_space(6.0);
    }

    fn render_group_card(&mut self, ui: &mut egui::Ui, group: &GroupReport) {
        let visible: Vec<&Issue> = group
            .issues
            .iter()
            .filter(|i| self.filter.matches(i.severity))
            .collect();
        // If filtering and this group has no matches AND no status
        // information worth showing, skip it entirely to reduce
        // noise.
        if visible.is_empty() && self.filter != SeverityFilter::All && group.manifest_ok {
            return;
        }

        let accent = group_accent(&group.issues);
        let is_collapsed = self.collapsed.contains(&group.slug);
        let (errors, warnings, infos) = count_issues(&group.issues);

        group_card(ui, accent, |ui| {
            ui.horizontal(|ui| {
                if disclosure_button(ui, !is_collapsed).clicked() {
                    if is_collapsed {
                        self.collapsed.remove(&group.slug);
                    } else {
                        self.collapsed.insert(group.slug.clone());
                    }
                }
                ui.label(egui::RichText::new(&group.slug).strong());
                ui.add_space(6.0);
                manifest_pill(ui, group.manifest_ok);
                count_pill(ui, &format!("{} mem", group.memory_count));
                severity_mini_pills(ui, errors, warnings, infos);
            });
            if !is_collapsed {
                ui.add_space(4.0);
                if visible.is_empty() {
                    ui.label(
                        egui::RichText::new("(no issues at current filter)")
                            .small()
                            .weak(),
                    );
                } else {
                    for issue in visible {
                        render_issue_row(ui, issue);
                    }
                }
            }
        });
        ui.add_space(6.0);
    }
}

fn render_tiles(ui: &mut egui::Ui, errors: usize, warnings: usize, infos: usize) {
    ui.horizontal(|ui| {
        tile(ui, errors, "Errors", severity_color("error"));
        ui.add_space(10.0);
        tile(ui, warnings, "Warnings", severity_color("warn"));
        ui.add_space(10.0);
        tile(ui, infos, "Infos", severity_color("info"));
    });
}

fn tile(ui: &mut egui::Ui, count: usize, label: &str, accent: egui::Color32) {
    let fill = egui::Color32::from_rgba_unmultiplied(accent.r(), accent.g(), accent.b(), 32);
    egui::Frame::new()
        .fill(fill)
        .stroke(egui::Stroke::new(1.0, accent))
        .corner_radius(egui::CornerRadius::same(6))
        .inner_margin(egui::Margin::symmetric(16, 10))
        .show(ui, |ui| {
            ui.vertical(|ui| {
                ui.label(
                    egui::RichText::new(count.to_string())
                        .size(26.0)
                        .strong()
                        .color(accent),
                );
                ui.label(egui::RichText::new(label).small().weak());
            });
        });
}

fn filter_pill(ui: &mut egui::Ui, label: &str, count: usize, active: bool) -> egui::Response {
    let text = format!("{label} ({count})");
    let visuals = ui.visuals();
    let (fill, stroke_color, fg) = if active {
        (
            visuals.selection.bg_fill,
            visuals.selection.stroke.color,
            visuals.strong_text_color(),
        )
    } else {
        (
            egui::Color32::TRANSPARENT,
            visuals.widgets.inactive.bg_stroke.color,
            visuals.widgets.inactive.fg_stroke.color,
        )
    };
    let response = egui::Frame::new()
        .fill(fill)
        .stroke(egui::Stroke::new(1.0, stroke_color))
        .corner_radius(egui::CornerRadius::same(12))
        .inner_margin(egui::Margin::symmetric(12, 4))
        .show(ui, |ui| {
            ui.label(egui::RichText::new(text).color(fg).strong().size(12.0));
        })
        .response;
    response.interact(egui::Sense::click())
}

fn group_card<R>(ui: &mut egui::Ui, accent: egui::Color32, body: impl FnOnce(&mut egui::Ui) -> R) {
    egui::Frame::group(ui.style())
        .stroke(egui::Stroke::new(
            1.0,
            ui.visuals().widgets.noninteractive.bg_stroke.color,
        ))
        .fill(ui.visuals().faint_bg_color)
        .inner_margin(egui::Margin {
            left: 12,
            right: 10,
            top: 8,
            bottom: 8,
        })
        .show(ui, |ui| {
            // Colored accent bar along the left edge. Painted into
            // the frame's first rect so the card visually groups the
            // rows that follow.
            let rect = ui.max_rect();
            let accent_rect = egui::Rect::from_min_size(
                rect.left_top() + egui::vec2(-8.0, 0.0),
                egui::vec2(3.0, rect.height()),
            );
            ui.painter().rect_filled(accent_rect, 1.5, accent);
            body(ui);
        });
}

fn disclosure_button(ui: &mut egui::Ui, open: bool) -> egui::Response {
    let glyph = if open {
        icons::CARET_DOWN
    } else {
        icons::CARET_RIGHT
    };
    ui.add(egui::Button::new(glyph).frame(false))
}

fn manifest_pill(ui: &mut egui::Ui, ok: bool) {
    let (text, color) = if ok {
        ("manifest ok", egui::Color32::from_rgb(120, 200, 120))
    } else {
        ("manifest BROKEN", severity_color("error"))
    };
    crate::ui::tag_pill::show(
        ui,
        text,
        egui::Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), 40),
        color,
    );
}

fn count_pill(ui: &mut egui::Ui, text: &str) {
    let v = ui.visuals();
    crate::ui::tag_pill::show(
        ui,
        text,
        v.widgets.inactive.weak_bg_fill,
        v.widgets.inactive.fg_stroke.color,
    );
}

fn severity_mini_pills(ui: &mut egui::Ui, errors: usize, warnings: usize, infos: usize) {
    if errors > 0 {
        crate::ui::tag_pill::show(
            ui,
            &format!("{errors} {}", icons::X_CIRCLE),
            egui::Color32::from_rgba_unmultiplied(220, 100, 100, 40),
            severity_color("error"),
        );
    }
    if warnings > 0 {
        crate::ui::tag_pill::show(
            ui,
            &format!("{warnings} {}", icons::WARNING),
            egui::Color32::from_rgba_unmultiplied(220, 180, 80, 40),
            severity_color("warn"),
        );
    }
    if infos > 0 {
        crate::ui::tag_pill::show(
            ui,
            &format!("{infos} {}", icons::INFO),
            egui::Color32::from_rgba_unmultiplied(140, 180, 220, 40),
            severity_color("info"),
        );
    }
}

fn render_issue_row(ui: &mut egui::Ui, issue: &Issue) {
    let accent = severity_color(issue.severity);
    let icon = severity_icon(issue.severity);
    ui.horizontal_wrapped(|ui| {
        ui.add_space(18.0); // indent under the card's left accent
        ui.colored_label(accent, icon);
        if let Some(slug) = &issue.slug {
            let v = ui.visuals();
            crate::ui::tag_pill::show(
                ui,
                slug,
                v.widgets.inactive.weak_bg_fill,
                v.widgets.inactive.fg_stroke.color,
            );
        }
        ui.label(&issue.message);
    });
}

fn severity_icon(severity: &str) -> &'static str {
    match severity {
        "error" => icons::X_CIRCLE,
        "warn" => icons::WARNING,
        _ => icons::INFO,
    }
}

fn severity_color(severity: &str) -> egui::Color32 {
    match severity {
        "error" => egui::Color32::from_rgb(220, 100, 100),
        "warn" => egui::Color32::from_rgb(220, 180, 80),
        _ => egui::Color32::from_rgb(140, 180, 220),
    }
}

/// Highest-severity color present in a set of issues. Neutral when
/// empty so the card still has a definite accent.
fn group_accent(issues: &[Issue]) -> egui::Color32 {
    let mut has_error = false;
    let mut has_warn = false;
    for issue in issues {
        match issue.severity {
            "error" => has_error = true,
            "warn" => has_warn = true,
            _ => {}
        }
    }
    if has_error {
        severity_color("error")
    } else if has_warn {
        severity_color("warn")
    } else if issues.is_empty() {
        egui::Color32::from_rgb(100, 140, 100)
    } else {
        severity_color("info")
    }
}

fn count_issues(issues: &[Issue]) -> (usize, usize, usize) {
    let mut errors = 0usize;
    let mut warnings = 0usize;
    let mut infos = 0usize;
    for issue in issues {
        match issue.severity {
            "error" => errors += 1,
            "warn" => warnings += 1,
            _ => infos += 1,
        }
    }
    (errors, warnings, infos)
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
