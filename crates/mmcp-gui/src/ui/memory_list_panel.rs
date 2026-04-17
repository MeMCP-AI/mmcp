//! Middle pane: scrollable list of memory slugs inside the selected
//! group.
//!
//! Clicking a slug updates the selection and requests the memory
//! body off the background worker if it is not already in the
//! viewer cache.

use eframe::egui;

use crate::runtime::BackgroundHandle;
use crate::runtime::task::BackgroundTask;
use crate::state::AppState;

pub fn show(ui: &mut egui::Ui, state: &mut AppState, background: &BackgroundHandle) {
    egui::Panel::left("mmcp_gui_memory_list")
        .default_size(240.0)
        .show_inside(ui, |ui| {
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
                    ui.label("Loading memories...");
                    return;
                }
            };

            if slugs.is_empty() {
                ui.label("No memories in this group.");
                return;
            }

            egui::ScrollArea::vertical().show(ui, |ui| {
                for slug in &slugs {
                    let selected = state.selection.memory.as_deref() == Some(slug.as_str());
                    let resp = ui.selectable_label(selected, slug);
                    if resp.clicked() && !selected {
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
