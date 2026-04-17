//! Top toolbar: global actions that are not tied to a specific
//! memory selection (sync / diagnose) plus the CRUD triggers that
//! are (New / Edit / Delete).
//!
//! Sync buttons are disabled when `state.sync.is_ready()` is false
//! (sync not configured or a sync op is in flight). Edit / Delete
//! require a memory to be selected AND already loaded into the
//! viewer cache — we never start an edit session against a memory
//! whose body hasn't been fetched yet.

use eframe::egui;

use crate::runtime::BackgroundHandle;
use crate::runtime::task::BackgroundTask;
use crate::state::AppState;
use crate::state::editor_buffer::EditorBuffer;
use crate::state::sync_status::{SyncOp, SyncStatus};

pub fn show(ui: &mut egui::Ui, state: &mut AppState, background: &BackgroundHandle) {
    egui::Panel::top("mmcp_gui_toolbar").show_inside(ui, |ui| {
        ui.horizontal(|ui| {
            ui.add_space(4.0);

            render_crud_buttons(ui, state);
            ui.separator();
            render_sync_buttons(ui, state, background);
            ui.separator();
            render_diagnose_button(ui, state, background);
        });
    });
}

fn render_crud_buttons(ui: &mut egui::Ui, state: &mut AppState) {
    let selected_group = state.selection.group;
    let selected_memory = state.selection.memory.clone();
    let has_loaded_memory = match (selected_group, selected_memory.as_deref()) {
        (Some(g), Some(s)) => state.viewer.get(&g, s).is_some(),
        _ => false,
    };
    let not_editing = state.editor.is_none();

    let new_enabled = selected_group.is_some() && not_editing;
    if ui
        .add_enabled(new_enabled, egui::Button::new("New"))
        .on_disabled_hover_text("select a group first")
        .clicked()
    {
        if let Some(group_id) = selected_group {
            state.editor = Some(EditorBuffer::for_new(group_id));
        }
    }

    let edit_enabled = has_loaded_memory && not_editing;
    if ui
        .add_enabled(edit_enabled, egui::Button::new("Edit"))
        .on_disabled_hover_text("select a loaded memory to edit")
        .clicked()
    {
        if let (Some(group_id), Some(slug)) = (selected_group, selected_memory.clone()) {
            if let Some(memory) = state.viewer.get(&group_id, &slug) {
                state.editor = Some(EditorBuffer::for_edit(group_id, slug, &memory));
            }
        }
    }

    let delete_enabled = has_loaded_memory && not_editing && state.pending_delete.is_none();
    if ui
        .add_enabled(delete_enabled, egui::Button::new("Delete"))
        .on_disabled_hover_text("select a memory to delete")
        .clicked()
    {
        if let (Some(group_id), Some(slug)) = (selected_group, selected_memory) {
            state.pending_delete = Some((group_id, slug));
        }
    }
}

fn render_sync_buttons(ui: &mut egui::Ui, state: &mut AppState, background: &BackgroundHandle) {
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
}

fn render_diagnose_button(ui: &mut egui::Ui, state: &mut AppState, background: &BackgroundHandle) {
    if ui.button("Diagnose").clicked() {
        state.diag_panel_open = true;
        state.diag_report = None;
        background.send(BackgroundTask::RunDiagnose);
    }
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
