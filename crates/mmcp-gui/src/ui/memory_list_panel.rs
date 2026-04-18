//! Middle pane: scrollable list of memory slugs inside the selected
//! group, each row prefixed with a coloured kind badge.
//!
//! Row shape: `[kind pill] slug` inside a single horizontal strip.
//! The `selectable_label` captures the click (keeping keyboard
//! navigation and egui's selection visuals intact); the pill sits
//! next to it as a decorative badge. Until a memory's frontmatter
//! has loaded, a neutral placeholder pill takes the same slot so
//! the slug text doesn't jump when loads complete.

use eframe::egui;

use crate::runtime::BackgroundHandle;
use crate::runtime::task::BackgroundTask;
use crate::state::AppState;
use crate::state::settings::KindDisplay;
use crate::ui::kind_glyph;

pub fn show(ui: &mut egui::Ui, state: &mut AppState, background: &BackgroundHandle) {
    egui::Panel::left("mmcp_gui_memory_list")
        .default_size(280.0)
        .show_inside(ui, |ui| {
            ui.add_space(4.0);
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
                    ui.label("Loading memories…");
                    return;
                }
            };

            if slugs.is_empty() {
                ui.label("No memories in this group.");
                return;
            }

            let mode = state.settings.kind_display;

            egui::ScrollArea::vertical().show(ui, |ui| {
                for slug in &slugs {
                    let selected = state.selection.memory.as_deref() == Some(slug.as_str());
                    let kind = state
                        .viewer
                        .get(&group_id, slug)
                        .map(|m| m.frontmatter.kind);
                    let clicked = render_row(ui, mode, kind, slug, selected);
                    if clicked && !selected {
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

/// Paint one list row: optional kind badge + selectable slug label.
/// Returns `true` when the slug label was clicked this frame.
fn render_row(
    ui: &mut egui::Ui,
    mode: KindDisplay,
    kind: Option<mmcp_core::memory::MemoryKind>,
    slug: &str,
    selected: bool,
) -> bool {
    ui.horizontal(|ui| {
        if mode != KindDisplay::Off {
            match kind {
                Some(k) => kind_glyph::render_prefix(ui, mode, k),
                None => kind_glyph::render_placeholder(ui, mode),
            }
        }
        ui.selectable_label(selected, slug).clicked()
    })
    .inner
}
