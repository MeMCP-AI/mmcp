//! Top-level `eframe::App` implementation.
//!
//! Owns the full layout (toolbar, two side panels, central viewer
//! or editor, status bar, floating diagnostics window, delete
//! confirmation modal) and the background worker handle. Each
//! frame: drain the worker outcome channel into `AppState`, render
//! the regions, and request a repaint shortly so background
//! responses don't wait for a UI input event to paint.
//!
//! The central area swaps between viewer and editor based on
//! `state.editor.is_some()`: editing replaces viewing, which avoids
//! the ambiguity of a side-by-side layout while a memory is being
//! edited.

use eframe::egui;

use crate::runtime::{BackgroundHandle, BackgroundTask, TaskOutcome};
use crate::state::AppState;
use crate::ui::memory_editor::EditorWidget;
use crate::ui::{
    ViewerWidget, delete_confirmation, diagnostics_panel, group_panel, memory_list_panel,
    status_bar, toolbar,
};

pub struct MmcpGuiApp {
    state: AppState,
    background: BackgroundHandle,
    viewer: ViewerWidget,
    editor: EditorWidget,
    /// Held so the tokio runtime lives as long as the window. Dropped
    /// after `eframe::run_native` returns, which cancels the worker.
    _runtime: tokio::runtime::Runtime,
}

impl MmcpGuiApp {
    pub fn new(background: BackgroundHandle, runtime: tokio::runtime::Runtime) -> Self {
        Self {
            state: AppState::default(),
            background,
            viewer: ViewerWidget::default(),
            editor: EditorWidget::default(),
            _runtime: runtime,
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
                    self.background.send(BackgroundTask::RefreshGroups);
                }
                TaskOutcome::SyncPushCompleted { drained } => {
                    toolbar::apply_push_completed(&mut self.state, drained);
                }
                TaskOutcome::SyncFailed { op, message } => {
                    tracing::warn!(op = op.as_str(), error = %message, "sync failed");
                    toolbar::apply_sync_failed(&mut self.state, op, message);
                }
                TaskOutcome::HealthChanged { online, reason } => {
                    self.state.reachability = if online {
                        crate::state::sync_reachability::SyncReachability::Online
                    } else {
                        crate::state::sync_reachability::SyncReachability::Offline {
                            reason: reason.unwrap_or_else(|| "unreachable".to_string()),
                        }
                    };
                }
                TaskOutcome::DiagnoseCompleted(report) => {
                    self.state.diag_report = Some(report);
                }
                TaskOutcome::MemoryCreated { group_id, slug } => {
                    tracing::info!(%group_id, %slug, "memory created");
                    self.state.editor = None;
                    self.state.selection.memory = Some(slug.clone());
                    self.state.memory_slugs.remove(&group_id);
                    self.background
                        .send(BackgroundTask::LoadMemoryList { group_id });
                    self.background
                        .send(BackgroundTask::LoadMemory { group_id, slug });
                }
                TaskOutcome::MemoryUpdated { group_id, slug } => {
                    tracing::info!(%group_id, %slug, "memory updated");
                    self.state.editor = None;
                    // Invalidate the viewer cache entry so the next
                    // render fetches the just-written body.
                    self.background
                        .send(BackgroundTask::LoadMemory { group_id, slug });
                }
                TaskOutcome::MemoryDeleted { group_id, slug } => {
                    tracing::info!(%group_id, %slug, "memory deleted");
                    if self.state.selection.memory.as_deref() == Some(slug.as_str()) {
                        self.state.selection.memory = None;
                    }
                    self.state.memory_slugs.remove(&group_id);
                    self.background
                        .send(BackgroundTask::LoadMemoryList { group_id });
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
        // from the left, then viewer / editor paints into what's
        // left.
        toolbar::show(ui, &mut self.state, &self.background);
        status_bar::show(ui, &self.state);
        group_panel::show(ui, &mut self.state, &self.background);
        memory_list_panel::show(ui, &mut self.state, &self.background);
        if self.state.editor.is_some() {
            self.editor.show(ui, &mut self.state, &self.background);
        } else {
            self.viewer.show(ui, &self.state);
        }

        // Floating / modal overlays render after the central area.
        diagnostics_panel::show(ui.ctx(), &mut self.state);
        delete_confirmation::show(ui.ctx(), &mut self.state, &self.background);

        // No unconditional repaint: the background worker wakes egui
        // via `ctx.request_repaint()` on every outcome, so idle
        // frames cost zero CPU.
    }
}
