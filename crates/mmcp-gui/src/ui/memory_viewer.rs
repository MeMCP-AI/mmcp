//! Right / central pane: frontmatter metadata + rendered markdown body.
//!
//! Owns a `CommonMarkCache` so the renderer doesn't re-parse the
//! markdown tree on every frame. The cache is long-lived (stored in
//! [`ViewerWidget`]), which matches `egui_commonmark`'s expected
//! lifecycle.
//!
//! Unlike the side panels, this widget paints directly into the
//! passed `Ui` — in the three-pane layout the leftover space after
//! both `Panel::left`s have claimed theirs IS the central area, and
//! eframe has already wrapped us in it before `App::ui` fires.

use eframe::egui;
use egui_commonmark::{CommonMarkCache, CommonMarkViewer};
use mmcp_core::memory::MemoryFrontmatter;

use crate::state::AppState;

#[derive(Default)]
pub struct ViewerWidget {
    commonmark: CommonMarkCache,
}

impl ViewerWidget {
    pub fn show(&mut self, ui: &mut egui::Ui, state: &AppState) {
        let Some(group) = state.selection.group else {
            Self::placeholder(ui, "Select a group to begin.");
            return;
        };
        let Some(slug) = state.selection.memory.as_deref() else {
            Self::placeholder(ui, "Select a memory to view its body.");
            return;
        };
        let Some(memory) = state.viewer.get(&group, slug) else {
            ui.spinner();
            ui.label("Loading memory body...");
            return;
        };

        egui::ScrollArea::vertical().show(ui, |ui| {
            Self::render_frontmatter(ui, &memory.frontmatter);
            ui.separator();
            CommonMarkViewer::new().show(ui, &mut self.commonmark, &memory.body);
        });

        if let Some(err) = &state.last_error {
            ui.separator();
            ui.colored_label(egui::Color32::RED, err);
        }
    }

    fn placeholder(ui: &mut egui::Ui, msg: &str) {
        ui.vertical_centered(|ui| {
            ui.add_space(32.0);
            ui.label(msg);
        });
    }

    fn render_frontmatter(ui: &mut egui::Ui, fm: &MemoryFrontmatter) {
        ui.heading(&fm.name);
        ui.label(&fm.description);
        ui.horizontal_wrapped(|ui| {
            ui.label(format!("kind: {}", fm.kind.as_str()));
            if fm.mandatory {
                ui.label("• mandatory");
            }
            if let Some(v) = &fm.version {
                ui.label(format!("• v{v}"));
            }
            for tag in &fm.tags {
                ui.label(format!("#{tag}"));
            }
        });
    }
}
