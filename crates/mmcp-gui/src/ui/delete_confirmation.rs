//! Modal confirmation for memory deletion.
//!
//! Deletion itself is commit-backed and recoverable through git
//! history, but the UX cost of a missed click is high enough that a
//! one-click guard is worth it. The dialog is driven by
//! `AppState.pending_delete`: `Some((group, slug))` shows it,
//! confirm fires a `DeleteMemory` task and clears the state; cancel
//! just clears the state.

use eframe::egui;

use crate::runtime::BackgroundHandle;
use crate::runtime::task::BackgroundTask;
use crate::state::AppState;

pub fn show(ctx: &egui::Context, state: &mut AppState, background: &BackgroundHandle) {
    let Some((group_id, slug)) = state.pending_delete.clone() else {
        return;
    };

    let mut open = true;
    let mut confirmed = false;
    let mut cancelled = false;

    egui::Window::new("Delete memory?")
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .open(&mut open)
        .show(ctx, |ui| {
            ui.label(format!("Delete memory '{slug}'?"));
            ui.label(
                "A commit records the deletion; the memory can be recovered from git history.",
            );
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if ui.button("Delete").clicked() {
                    confirmed = true;
                }
                if ui.button("Cancel").clicked() {
                    cancelled = true;
                }
            });
        });

    if confirmed {
        background.send(BackgroundTask::DeleteMemory {
            group_id,
            slug: slug.clone(),
        });
        state.pending_delete = None;
    } else if cancelled || !open {
        state.pending_delete = None;
    }
}
