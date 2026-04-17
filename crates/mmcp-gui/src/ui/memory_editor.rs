//! Central pane — edit mode.
//!
//! Replaces the viewer when `state.editor.is_some()`. Shape:
//!
//! 1. Header ("New memory" / "Edit memory")
//! 2. Frontmatter form in a card (grid-aligned labels + inputs)
//! 3. Body section split side-by-side: raw markdown editor on the
//!    left, live rendered preview on the right, each with its own
//!    vertical scroll. The split means a reviewer sees both the
//!    source and the rendered output without toggling modes.
//! 4. Action row (Create / Save, Cancel) with inline validation.
//!
//! Owns a `CommonMarkCache` for the preview so each keystroke
//! doesn't re-parse the full markdown AST from scratch.

use eframe::egui;
use egui_commonmark::CommonMarkCache;
use mmcp_core::id::GroupId;
use mmcp_core::memory::{MemoryFile, MemoryKind};

use crate::runtime::BackgroundHandle;
use crate::runtime::task::BackgroundTask;
use crate::state::AppState;
use crate::state::editor_buffer::{EditorBuffer, EditorMode};
use crate::ui::markdown_render;

/// Height reserved for the body split. Tuned so the editor is tall
/// enough to show ~24 lines of monospace text at 13 pt without
/// forcing a scroll on a 1200×800 window.
const BODY_SPLIT_HEIGHT: f32 = 420.0;
/// Gap between the source column and the preview column.
const SPLIT_GAP: f32 = 12.0;

#[derive(Default)]
pub struct EditorWidget {
    preview_cache: CommonMarkCache,
}

impl EditorWidget {
    pub fn show(&mut self, ui: &mut egui::Ui, state: &mut AppState, background: &BackgroundHandle) {
        if state.editor.is_none() {
            return;
        }
        egui::ScrollArea::vertical()
            .id_salt("mmcp_gui_editor_outer")
            .show(ui, |ui| {
                if let Some(buffer) = state.editor.as_mut() {
                    render_header(ui, buffer);
                    ui.add_space(8.0);
                    render_frontmatter_card(ui, buffer);
                    ui.add_space(12.0);
                    self.render_body_split(ui, buffer);
                    ui.add_space(12.0);
                }
                render_actions(ui, state, background);
            });
    }

    fn render_body_split(&mut self, ui: &mut egui::Ui, buffer: &mut EditorBuffer) {
        ui.label(egui::RichText::new("BODY (MARKDOWN)").small().strong());
        ui.add_space(4.0);

        let total_width = ui.available_width();
        let col_width = ((total_width - SPLIT_GAP) / 2.0).max(220.0);
        let cache = &mut self.preview_cache;

        ui.horizontal_top(|ui| {
            ui.allocate_ui_with_layout(
                egui::vec2(col_width, BODY_SPLIT_HEIGHT),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    render_source_column(ui, buffer);
                },
            );
            ui.add_space(SPLIT_GAP);
            ui.allocate_ui_with_layout(
                egui::vec2(col_width, BODY_SPLIT_HEIGHT),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    render_preview_column(ui, &buffer.body, cache);
                },
            );
        });
    }
}

fn render_header(ui: &mut egui::Ui, buffer: &EditorBuffer) {
    let title = match buffer.mode {
        EditorMode::New => "New memory",
        EditorMode::Edit => "Edit memory",
    };
    ui.heading(title);
}

fn render_frontmatter_card(ui: &mut egui::Ui, buffer: &mut EditorBuffer) {
    egui::Frame::group(ui.style())
        .inner_margin(egui::Margin::same(12))
        .show(ui, |ui| {
            egui::Grid::new("mmcp_gui_editor_form")
                .num_columns(2)
                .spacing([12.0, 10.0])
                .min_col_width(110.0)
                .show(ui, |ui| {
                    ui.label("slug");
                    ui.add_enabled_ui(buffer.mode == EditorMode::New, |ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut buffer.slug)
                                .desired_width(f32::INFINITY)
                                .hint_text("kebab-case, e.g. rule-commit-format"),
                        );
                    });
                    ui.end_row();

                    ui.label("name");
                    ui.add(
                        egui::TextEdit::singleline(&mut buffer.name).desired_width(f32::INFINITY),
                    );
                    ui.end_row();

                    ui.label("description");
                    ui.add(
                        egui::TextEdit::singleline(&mut buffer.description)
                            .desired_width(f32::INFINITY),
                    );
                    ui.end_row();

                    ui.label("kind");
                    kind_combo(ui, &mut buffer.kind);
                    ui.end_row();

                    ui.label("mandatory");
                    ui.checkbox(&mut buffer.mandatory, "");
                    ui.end_row();

                    ui.label("tags");
                    ui.add(
                        egui::TextEdit::singleline(&mut buffer.tags_raw)
                            .desired_width(f32::INFINITY)
                            .hint_text("comma-separated"),
                    );
                    ui.end_row();
                });
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
                MemoryKind::Feature,
            ] {
                ui.selectable_value(kind, candidate, candidate.as_str());
            }
        });
}

fn render_source_column(ui: &mut egui::Ui, buffer: &mut EditorBuffer) {
    ui.label(egui::RichText::new("source").small().weak());
    egui::Frame::group(ui.style())
        .inner_margin(egui::Margin::same(6))
        .show(ui, |ui| {
            egui::ScrollArea::vertical()
                .id_salt("mmcp_gui_editor_source")
                .max_height(BODY_SPLIT_HEIGHT - 28.0)
                .show(ui, |ui| {
                    ui.add(
                        egui::TextEdit::multiline(&mut buffer.body)
                            .desired_width(f32::INFINITY)
                            .desired_rows(24)
                            .font(egui::TextStyle::Monospace)
                            .code_editor(),
                    );
                });
        });
}

fn render_preview_column(ui: &mut egui::Ui, body: &str, cache: &mut CommonMarkCache) {
    ui.label(egui::RichText::new("preview").small().weak());
    egui::Frame::group(ui.style())
        .fill(ui.visuals().extreme_bg_color)
        .inner_margin(egui::Margin::same(10))
        .show(ui, |ui| {
            egui::ScrollArea::vertical()
                .id_salt("mmcp_gui_editor_preview")
                .max_height(BODY_SPLIT_HEIGHT - 28.0)
                .show(ui, |ui| {
                    markdown_render::render(ui, cache, body);
                });
        });
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
