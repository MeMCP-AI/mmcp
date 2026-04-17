//! Top-level `eframe::App` implementation.
//!
//! Owns the three-pane layout and the background worker handle.
//! Each frame: drain the worker outcome channel into `AppState`,
//! render the three panels, and request a repaint shortly so that
//! background responses don't wait for a UI input event to paint.
//!
//! The layout is built entirely inside the single `ui` handle eframe
//! hands us — two `Panel::left` calls claim space from the left,
//! then the viewer paints directly into the leftover central area.

use eframe::egui;

use crate::runtime::{BackgroundHandle, BackgroundTask, TaskOutcome};
use crate::state::AppState;
use crate::ui::{ViewerWidget, group_panel, memory_list_panel};

pub struct MmcpGuiApp {
    state: AppState,
    background: BackgroundHandle,
    viewer: ViewerWidget,
}

impl MmcpGuiApp {
    pub fn new(background: BackgroundHandle) -> Self {
        Self {
            state: AppState::default(),
            background,
            viewer: ViewerWidget::default(),
        }
    }

    fn drain_outcomes(&mut self) {
        while let Some(outcome) = self.background.try_recv() {
            match outcome {
                TaskOutcome::GroupsRefreshed(groups) => {
                    self.state.groups = groups;
                }
                TaskOutcome::MemoryListLoaded { group_id, slugs } => {
                    self.state.memory_slugs.insert(group_id, slugs);
                }
                TaskOutcome::MemoryLoaded {
                    group_id,
                    slug,
                    memory,
                } => {
                    self.state.viewer.insert(group_id, slug, memory);
                }
                TaskOutcome::Error(msg) => {
                    tracing::warn!(error = %msg, "background task failed");
                    self.state.last_error = Some(msg);
                }
            }
        }
    }

    /// Convenience: request a fresh group index from the worker.
    /// Currently unused by the phase-2 UI; wired up in phase 3 when
    /// the toolbar lands.
    #[allow(dead_code)]
    pub fn request_refresh(&self) {
        self.background.send(BackgroundTask::RefreshGroups);
    }
}

impl eframe::App for MmcpGuiApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.drain_outcomes();

        group_panel::show(ui, &mut self.state, &self.background);
        memory_list_panel::show(ui, &mut self.state, &self.background);
        self.viewer.show(ui, &self.state);

        // Poll the worker while any task is plausibly in-flight.
        // Without this, egui repaints only on input events, so a
        // background response wouldn't paint until the user moved
        // the mouse. A 100 ms hint is frequent enough to feel
        // instant and rare enough to keep CPU idle.
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(100));
    }
}
