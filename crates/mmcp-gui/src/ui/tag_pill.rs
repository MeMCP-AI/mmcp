//! Small rounded label used for tags, kinds, and status pips.
//!
//! Keeps the visual vocabulary consistent across the viewer's tag
//! row, the memory-list kind badges, and any future status pills
//! (mandatory, protected, etc.). All pills share the same corner
//! radius, padding, and font size so the eye groups them without
//! having to parse each site's ad-hoc formatting.

use eframe::egui;

pub fn show(ui: &mut egui::Ui, text: &str, fill: egui::Color32, text_color: egui::Color32) {
    egui::Frame::new()
        .fill(fill)
        .corner_radius(egui::CornerRadius::same(10))
        .inner_margin(egui::Margin::symmetric(8, 2))
        .show(ui, |ui| {
            ui.label(
                egui::RichText::new(text)
                    .color(text_color)
                    .size(11.0)
                    .strong(),
            );
        });
}

/// Neutral pill for generic tags.
pub fn tag(ui: &mut egui::Ui, text: &str) {
    let v = ui.visuals();
    show(
        ui,
        text,
        v.widgets.inactive.weak_bg_fill,
        v.widgets.inactive.fg_stroke.color,
    );
}

/// Accent pill for the memory kind.
pub fn kind(ui: &mut egui::Ui, text: &str) {
    show(
        ui,
        text,
        egui::Color32::from_rgb(50, 70, 100),
        egui::Color32::from_rgb(190, 210, 240),
    );
}

/// Warning pill for mandatory memories.
pub fn mandatory(ui: &mut egui::Ui) {
    show(
        ui,
        "mandatory",
        egui::Color32::from_rgb(100, 70, 40),
        egui::Color32::from_rgb(240, 210, 170),
    );
}
