//! Left pane: scrollable list of every group in the local mirror.
//!
//! Clicking a group updates [`crate::state::app_state::AppState::selection`] and kicks
//! off a `LoadMemoryList` task so the middle pane has data to show
//! by the time the frame completes.

use eframe::egui;

use crate::runtime::BackgroundHandle;
use crate::runtime::task::BackgroundTask;
use crate::state::AppState;

pub fn show(ui: &mut egui::Ui, state: &mut AppState, background: &BackgroundHandle) {
    egui::Panel::left("mmcp_gui_groups")
        .default_size(200.0)
        .show_inside(ui, |ui| {
            ui.heading("Groups");
            ui.separator();

            if state.groups.is_empty() {
                ui.label("No groups in local mirror.");
                ui.label("Run `mmcp init project` to create one.");
                return;
            }

            egui::ScrollArea::vertical().show(ui, |ui| {
                // Clone the minimal identifying slice so we can pass
                // `state` mutably to the click handler without
                // holding an iterator borrow into `state.groups`.
                let rows: Vec<(mmcp_core::id::GroupId, String)> = state
                    .groups
                    .iter()
                    .map(|e| {
                        (
                            e.manifest.group_id,
                            e.manifest
                                .display_name
                                .clone()
                                .unwrap_or_else(|| e.manifest.slug.clone()),
                        )
                    })
                    .collect();

                for (group_id, label) in rows {
                    let selected = state.selection.group == Some(group_id);
                    let resp = ui.selectable_label(selected, label);
                    if resp.clicked() && !selected {
                        state.selection.group = Some(group_id);
                        state.selection.memory = None;
                        background.send(BackgroundTask::LoadMemoryList { group_id });
                    }
                }
            });
        });
}
