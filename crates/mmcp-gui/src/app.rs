//! Top-level `eframe::App` implementation.
//!
//! Owns the four-region layout (toolbar, two side panels, central
//! viewer, status bar) and the background worker handle. Each
//! frame: drain the worker outcome channel into `AppState`, render
//! the regions, and request a repaint shortly so that background
//! responses don't wait for a UI input event to paint.

use eframe::egui;

use crate::runtime::{BackgroundHandle, BackgroundTask, TaskOutcome};
use crate::state::AppState;
use crate::ui::{
    ViewerWidget, diagnostics_panel, group_panel, memory_list_panel, status_bar, toolbar,
};

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
                TaskOutcome::SyncAvailable { server_url } => {
                    toolbar::apply_sync_available(&mut self.state, server_url);
                }
                TaskOutcome::SyncPullCompleted {
                    updated,
                    new_groups,
                } => {
                    toolbar::apply_pull_completed(&mut self.state, updated, new_groups);
                    // After a successful pull, mirrored groups may
                    // have shifted; refresh so the left pane picks
                    // up new groups or removed ones.
                    self.background.send(BackgroundTask::RefreshGroups);
                }
                TaskOutcome::SyncPushCompleted { drained } => {
                    toolbar::apply_push_completed(&mut self.state, drained);
                }
                TaskOutcome::SyncFailed { op, message } => {
                    tracing::warn!(op = op.as_str(), error = %message, "sync failed");
                    toolbar::apply_sync_failed(&mut self.state, op, message);
                }
                TaskOutcome::DiagnoseCompleted(report) => {
                    self.state.diag_report = Some(report);
                }
                TaskOutcome::Error(msg) => {
                    tracing::warn!(error = %msg, "background task failed");
                    self.state.last_error = Some(msg);
                }
            }
        }
    }
}

impl eframe::App for MmcpGuiApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.drain_outcomes();

        // Order matters: outermost panels claim space first. Toolbar
        // on top, status bar on bottom, then groups and memory list
        // from the left, then the viewer paints into what's left.
        toolbar::show(ui, &mut self.state, &self.background);
        status_bar::show(ui, &self.state);
        group_panel::show(ui, &mut self.state, &self.background);
        memory_list_panel::show(ui, &mut self.state, &self.background);
        self.viewer.show(ui, &self.state);
        diagnostics_panel::show(ui.ctx(), &mut self.state);

        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(100));
    }
}
