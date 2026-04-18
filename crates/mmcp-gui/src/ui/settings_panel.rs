//! Floating settings window.
//!
//! One row per preference: label on the left, control on the right.
//! New settings slot in as additional grid rows — no architectural
//! change needed. Driven by `AppState.settings_panel_open`; dismissed
//! by the window's native close button or by toggling the toolbar
//! gear again.

use eframe::egui;
use mmcp_core::memory::MemoryKind;

use crate::state::AppState;
use crate::state::settings::KindDisplay;
use crate::ui::kind_glyph;

pub fn show(ctx: &egui::Context, state: &mut AppState) {
    if !state.settings_panel_open {
        return;
    }
    let mut open = state.settings_panel_open;

    egui::Window::new("Settings")
        .collapsible(false)
        .resizable(false)
        .default_size([460.0, 280.0])
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .open(&mut open)
        .show(ctx, |ui| {
            render_body(ui, state);
            ui.separator();
            render_footer(ui);
        });

    state.settings_panel_open = open;
}

fn render_body(ui: &mut egui::Ui, state: &mut AppState) {
    egui::Grid::new("mmcp_gui_settings_grid")
        .num_columns(2)
        .spacing([16.0, 12.0])
        .min_col_width(170.0)
        .show(ui, |ui| {
            ui.label(egui::RichText::new("Memory list prefix").strong());
            render_kind_display_radio(ui, &mut state.settings.kind_display);
            ui.end_row();

            ui.label(egui::RichText::new("Preview").small().weak());
            render_prefix_preview(ui, state.settings.kind_display);
            ui.end_row();
        });
}

fn render_kind_display_radio(ui: &mut egui::Ui, value: &mut KindDisplay) {
    ui.vertical(|ui| {
        for option in [
            KindDisplay::Off,
            KindDisplay::Icon,
            KindDisplay::Text,
            KindDisplay::IconAndText,
        ] {
            ui.radio_value(value, option, option.label());
        }
    });
}

/// One sample row per kind, rendered exactly as the memory list
/// would. Makes the difference between `Icon` / `Text` /
/// `Icon + text` immediately obvious without having to toggle back
/// and forth.
fn render_prefix_preview(ui: &mut egui::Ui, mode: KindDisplay) {
    egui::Frame::group(ui.style())
        .inner_margin(egui::Margin::same(8))
        .show(ui, |ui| {
            ui.vertical(|ui| {
                for (kind, sample_slug) in [
                    (MemoryKind::Rule, "branch-policy"),
                    (MemoryKind::Snapshot, "repo-state-2026-04-18"),
                    (MemoryKind::Log, "incident-2026-03-05"),
                    (MemoryKind::Reference, "gitoxide-upstream"),
                    (MemoryKind::Scratch, "draft-notes"),
                    (MemoryKind::Feature, "fr-020-extract-mmcp-store"),
                ] {
                    ui.horizontal(|ui| {
                        if mode != KindDisplay::Off {
                            kind_glyph::render_prefix(ui, mode, kind);
                        }
                        ui.label(sample_slug);
                    });
                }
            });
        });
}

fn render_footer(ui: &mut egui::Ui) {
    ui.label(
        egui::RichText::new(
            "Settings persist to the platform's app-config dir \
             (~/.config/mmcp-gui on Linux, %APPDATA%\\mmcp-gui on Windows). \
             Delete that file to reset.",
        )
        .small()
        .weak(),
    );
}
