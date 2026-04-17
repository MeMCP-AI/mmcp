//! Right / central pane: frontmatter metadata + rendered markdown body.
//!
//! The frontmatter header is wrapped in a Frame-styled "card" so it
//! reads as a distinct metadata region rather than a floating label
//! run. Tags, kind, and the mandatory flag are rendered as pills via
//! the shared [`crate::ui::tag_pill`] helper so the visual vocabulary
//! stays consistent with the memory list's kind badges.
//!
//! Owns a `CommonMarkCache` so the renderer doesn't re-parse the
//! markdown tree on every frame. The cache is long-lived (stored in
//! [`ViewerWidget`]), which matches `egui_commonmark`'s expected
//! lifecycle.

use eframe::egui;
use egui_commonmark::{CommonMarkCache, CommonMarkViewer};
use mmcp_core::memory::MemoryFrontmatter;

use crate::state::AppState;
use crate::ui::tag_pill;

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
            ui.label("Loading memory body…");
            return;
        };

        egui::ScrollArea::vertical().show(ui, |ui| {
            Self::render_frontmatter_card(ui, &memory.frontmatter, slug);
            ui.add_space(12.0);
            CommonMarkViewer::new().show(ui, &mut self.commonmark, &memory.body);
        });

        if let Some(err) = &state.last_error {
            ui.separator();
            ui.colored_label(egui::Color32::from_rgb(220, 120, 120), err);
        }
    }

    fn placeholder(ui: &mut egui::Ui, msg: &str) {
        ui.vertical_centered(|ui| {
            ui.add_space(32.0);
            ui.label(msg);
        });
    }

    fn render_frontmatter_card(ui: &mut egui::Ui, fm: &MemoryFrontmatter, slug: &str) {
        egui::Frame::group(ui.style())
            .inner_margin(egui::Margin::same(12))
            .show(ui, |ui| {
                ui.heading(&fm.name);
                ui.label(
                    egui::RichText::new(slug)
                        .small()
                        .color(ui.visuals().widgets.inactive.fg_stroke.color),
                );
                ui.add_space(4.0);
                ui.label(&fm.description);
                ui.add_space(8.0);

                // Pill row: kind + optional mandatory + version + tags.
                ui.horizontal_wrapped(|ui| {
                    tag_pill::kind(ui, fm.kind.as_str());
                    if fm.mandatory {
                        tag_pill::mandatory(ui);
                    }
                    if let Some(v) = &fm.version {
                        tag_pill::tag(ui, &format!("v{v}"));
                    }
                    for tag in &fm.tags {
                        tag_pill::tag(ui, tag);
                    }
                });
            });
    }
}
