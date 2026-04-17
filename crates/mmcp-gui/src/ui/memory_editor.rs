//! Central pane — edit mode.
//!
//! Replaces the viewer when `state.editor.is_some()`. Render shape:
//! a stack of labeled form rows for the frontmatter, a tall
//! multiline editor for the body, and a Save / Cancel button row.
//!
//! Save fires a `CreateMemory` or `UpdateMemory` task based on the
//! buffer's mode; the outcome drain clears the editor state and
//! requests a memory-list refresh so the middle pane picks up the
//! new or changed slug.

use eframe::egui;
use mmcp_core::id::GroupId;
use mmcp_core::memory::{MemoryFile, MemoryKind};

use crate::runtime::BackgroundHandle;
use crate::runtime::task::BackgroundTask;
use crate::state::AppState;
use crate::state::editor_buffer::{EditorBuffer, EditorMode};

pub fn show(ui: &mut egui::Ui, state: &mut AppState, background: &BackgroundHandle) {
    if state.editor.is_none() {
        return;
    }
    egui::ScrollArea::vertical().show(ui, |ui| {
        // Scoped borrow of the buffer for the form sections so the
        // subsequent `render_actions` call can re-borrow `state`.
        if let Some(buffer) = state.editor.as_mut() {
            render_header(ui, buffer);
            ui.separator();
            render_frontmatter_form(ui, buffer);
            ui.separator();
            render_body_editor(ui, buffer);
            ui.separator();
        }
        render_actions(ui, state, background);
    });
}

fn render_header(ui: &mut egui::Ui, buffer: &EditorBuffer) {
    let title = match buffer.mode {
        EditorMode::New => "New memory",
        EditorMode::Edit => "Edit memory",
    };
    ui.heading(title);
}

fn render_frontmatter_form(ui: &mut egui::Ui, buffer: &mut EditorBuffer) {
    egui::Grid::new("mmcp_gui_editor_form")
        .num_columns(2)
        .spacing([12.0, 8.0])
        .show(ui, |ui| {
            ui.label("slug");
            ui.add_enabled_ui(buffer.mode == EditorMode::New, |ui| {
                ui.text_edit_singleline(&mut buffer.slug);
            });
            ui.end_row();

            ui.label("name");
            ui.text_edit_singleline(&mut buffer.name);
            ui.end_row();

            ui.label("description");
            ui.text_edit_singleline(&mut buffer.description);
            ui.end_row();

            ui.label("kind");
            kind_combo(ui, &mut buffer.kind);
            ui.end_row();

            ui.label("mandatory");
            ui.checkbox(&mut buffer.mandatory, "");
            ui.end_row();

            ui.label("tags");
            ui.text_edit_singleline(&mut buffer.tags_raw)
                .on_hover_text("comma-separated");
            ui.end_row();
        });
}

fn kind_combo(ui: &mut egui::Ui, kind: &mut MemoryKind) {
    egui::ComboBox::from_id_salt("mmcp_gui_editor_kind")
        .selected_text(kind.as_str())
        .show_ui(ui, |ui| {
            for candidate in [
                MemoryKind::Rule,
                MemoryKind::Snapshot,
                MemoryKind::Log,
                MemoryKind::Reference,
                MemoryKind::Scratch,
                MemoryKind::Fr,
            ] {
                ui.selectable_value(kind, candidate, candidate.as_str());
            }
        });
}

fn render_body_editor(ui: &mut egui::Ui, buffer: &mut EditorBuffer) {
    ui.label("body (markdown)");
    ui.add(
        egui::TextEdit::multiline(&mut buffer.body)
            .desired_rows(18)
            .desired_width(f32::INFINITY)
            .code_editor(),
    );
}

struct EditorSnapshot {
    validation: Option<&'static str>,
    mode: EditorMode,
    group_id: GroupId,
    slug: String,
    file: MemoryFile,
}

fn render_actions(ui: &mut egui::Ui, state: &mut AppState, background: &BackgroundHandle) {
    let snapshot = match &state.editor {
        None => return,
        Some(b) => EditorSnapshot {
            validation: b.validation_error(),
            mode: b.mode,
            group_id: b.group_id,
            slug: b.slug.clone(),
            file: b.to_memory_file(),
        },
    };

    let mut save_clicked = false;
    let mut cancel_clicked = false;

    ui.horizontal(|ui| {
        let save_enabled = snapshot.validation.is_none();
        let save_label = match snapshot.mode {
            EditorMode::New => "Create",
            EditorMode::Edit => "Save",
        };
        if ui
            .add_enabled(save_enabled, egui::Button::new(save_label))
            .clicked()
        {
            save_clicked = true;
        }
        if ui.button("Cancel").clicked() {
            cancel_clicked = true;
        }
        if let Some(msg) = snapshot.validation {
            ui.colored_label(egui::Color32::from_rgb(220, 120, 120), msg);
        }
    });

    if save_clicked {
        let task = match snapshot.mode {
            EditorMode::New => BackgroundTask::CreateMemory {
                group_id: snapshot.group_id,
                slug: snapshot.slug.trim().to_string(),
                memory: snapshot.file,
            },
            EditorMode::Edit => BackgroundTask::UpdateMemory {
                group_id: snapshot.group_id,
                slug: snapshot.slug,
                memory: snapshot.file,
            },
        };
        background.send(task);
    }
    if cancel_clicked {
        state.editor = None;
    }
}
