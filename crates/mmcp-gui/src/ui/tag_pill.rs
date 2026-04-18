//! Small rounded label used for tags, kinds, and status pips.
//!
//! Keeps the visual vocabulary consistent across the viewer's tag
//! row, the memory-list kind badges, and any future status pills
//! (mandatory, protected, etc.). All pills share the same corner
//! radius, padding, and font size so the eye groups them without
//! having to parse each site's ad-hoc formatting.

use eframe::egui;

/// Low-level pill painter. Returns the widget's `Response` so
/// callers can wire click / hover behaviour when they want the pill
/// to be interactive (e.g. the memory-list kind badge).
pub fn show(
    ui: &mut egui::Ui,
    text: &str,
    fill: egui::Color32,
    text_color: egui::Color32,
) -> egui::Response {
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
        })
        .response
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

/// Coloured pill used by kind badges: translucent fill keyed to
/// `accent`, text painted in solid `accent`. Produces a chip that
/// stands out against the dark panel without shouting over the
/// surrounding text.
pub fn kind_colored(ui: &mut egui::Ui, text: &str, accent: egui::Color32) -> egui::Response {
    let fill = egui::Color32::from_rgba_unmultiplied(accent.r(), accent.g(), accent.b(), 48);
    show(ui, text, fill, accent)
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
