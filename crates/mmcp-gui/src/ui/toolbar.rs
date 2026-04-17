//! Top toolbar: global actions that are not tied to a specific
//! memory selection.
//!
//! Phase 3 ships the Pull / Push buttons. Phase 5 will add
//! New / Edit / Delete once the write path lands. Buttons
//! responsible for sync are disabled when `AppState.sync.is_ready()`
//! is false (sync not configured or a sync op is in flight).

use eframe::egui;

use crate::runtime::BackgroundHandle;
use crate::runtime::task::BackgroundTask;
use crate::state::AppState;
use crate::state::sync_status::{SyncOp, SyncStatus};

pub fn show(ui: &mut egui::Ui, state: &mut AppState, background: &BackgroundHandle) {
    egui::Panel::top("mmcp_gui_toolbar").show_inside(ui, |ui| {
        ui.horizontal(|ui| {
            ui.add_space(4.0);

            let ready = state.sync.is_ready();
            if ui
                .add_enabled(ready, egui::Button::new("Pull"))
                .on_disabled_hover_text(disabled_hint(&state.sync, SyncOp::Pull))
                .clicked()
            {
                begin_sync(state, SyncOp::Pull);
                background.send(BackgroundTask::SyncPull);
            }
            if ui
                .add_enabled(ready, egui::Button::new("Push"))
                .on_disabled_hover_text(disabled_hint(&state.sync, SyncOp::Push))
                .clicked()
            {
                begin_sync(state, SyncOp::Push);
                background.send(BackgroundTask::SyncPush);
            }

            ui.separator();

            if ui.button("Diagnose").clicked() {
                state.diag_panel_open = true;
                state.diag_report = None;
                background.send(BackgroundTask::RunDiagnose);
            }
        });
    });
}

fn begin_sync(state: &mut AppState, op: SyncOp) {
    if let Some(server_url) = state.sync.server_url().map(str::to_string) {
        state.sync = SyncStatus::Syncing { server_url, op };
    }
}

fn disabled_hint(status: &SyncStatus, op: SyncOp) -> String {
    match status {
        SyncStatus::Unknown => "starting up…".to_string(),
        SyncStatus::NotConfigured => {
            "sync is not configured for this project — add a [sync] block to .mmcp.toml".to_string()
        }
        SyncStatus::Syncing { op: in_flight, .. } => {
            format!("{} already in progress", in_flight.as_str())
        }
        _ => format!("{} available", op.as_str()),
    }
}

/// Apply a sync outcome to `AppState.sync`. Called from the outcome
/// drain in `app.rs` so the toolbar and status bar agree on the
/// next state.
pub fn apply_pull_completed(state: &mut AppState, updated: usize, new_groups: usize) {
    let server_url = state
        .sync
        .server_url()
        .map(str::to_string)
        .unwrap_or_default();
    state.sync = SyncStatus::LastOk {
        server_url,
        op: SyncOp::Pull,
        summary: format!("{updated} updated, {new_groups} new"),
    };
}

pub fn apply_push_completed(state: &mut AppState, drained: usize) {
    let server_url = state
        .sync
        .server_url()
        .map(str::to_string)
        .unwrap_or_default();
    state.sync = SyncStatus::LastOk {
        server_url,
        op: SyncOp::Push,
        summary: format!("{drained} edit(s) pushed"),
    };
}

pub fn apply_sync_failed(state: &mut AppState, op: SyncOp, message: String) {
    let server_url = state
        .sync
        .server_url()
        .map(str::to_string)
        .unwrap_or_default();
    state.sync = SyncStatus::LastErr {
        server_url,
        op,
        message,
    };
}

pub fn apply_sync_available(state: &mut AppState, server_url: Option<String>) {
    state.sync = match server_url {
        Some(url) => SyncStatus::Idle { server_url: url },
        None => SyncStatus::NotConfigured,
    };
}
