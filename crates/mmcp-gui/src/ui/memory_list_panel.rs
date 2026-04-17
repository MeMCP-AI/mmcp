//! Middle pane: scrollable list of memory slugs inside the selected
//! group, each row prefixed with a kind indicator.
//!
//! The prefix format is user-configurable via
//! `AppState.settings.kind_display` (Off / Icon / Text / Icon+Text)
//! and falls back to a neutral placeholder while a memory's
//! frontmatter is still loading. On group select the app triggers
//! a sequential LoadMemory for every unknown slug so the prefixes
//! converge to their real kind as outcomes land.

use eframe::egui;

use crate::runtime::BackgroundHandle;
use crate::runtime::task::BackgroundTask;
use crate::state::AppState;
use crate::ui::kind_glyph;

pub fn show(ui: &mut egui::Ui, state: &mut AppState, background: &BackgroundHandle) {
    egui::Panel::left("mmcp_gui_memory_list")
        .default_size(260.0)
        .show_inside(ui, |ui| {
            ui.add_space(4.0);
            ui.heading("Memories");
            ui.separator();

            let Some(group_id) = state.selection.group else {
                ui.label("Select a group on the left.");
                return;
            };

            let slugs = match state.memory_slugs.get(&group_id) {
                Some(s) => s.clone(),
                None => {
                    ui.spinner();
                    ui.label("Loading memories…");
                    return;
                }
            };

            if slugs.is_empty() {
                ui.label("No memories in this group.");
                return;
            }

            let mode = state.settings.kind_display;

            egui::ScrollArea::vertical().show(ui, |ui| {
                for slug in &slugs {
                    let selected = state.selection.memory.as_deref() == Some(slug.as_str());
                    let kind = state
                        .viewer
                        .get(&group_id, slug)
                        .map(|m| m.frontmatter.kind);
                    let label = match kind {
                        Some(k) => format!("{}{slug}", kind_glyph::prefix_for(mode, k)),
                        None => format!("{}{slug}", kind_glyph::placeholder_prefix(mode)),
                    };
                    if ui.selectable_label(selected, label).clicked() && !selected {
                        state.selection.memory = Some(slug.clone());
                        if state.viewer.get(&group_id, slug).is_none() {
                            background.send(BackgroundTask::LoadMemory {
                                group_id,
                                slug: slug.clone(),
                            });
                        }
                    }
                }
            });
        });
}
