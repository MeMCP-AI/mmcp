//! Top-level `eframe::App` implementation.
//!
//! Phase 1 is intentionally empty — it renders a placeholder central
//! panel so the scaffolding commit stays minimal and the workspace
//! integration can be verified in isolation. Subsequent phases wire
//! in the group / memory / viewer / editor panels.

use eframe::egui;

#[derive(Default)]
pub struct MmcpGuiApp;

impl eframe::App for MmcpGuiApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        ui.vertical_centered(|ui| {
            ui.add_space(16.0);
            ui.heading("mmcp-gui");
            ui.add_space(8.0);
            ui.label("Desktop visual client for local memory stores.");
            ui.add_space(4.0);
            ui.label("Phase 1 bootstrap — no data wired yet.");
        });
    }
}
