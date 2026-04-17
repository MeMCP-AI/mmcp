//! Global egui style preset applied once at startup.
//!
//! egui's defaults ship a light theme with tight spacing and small
//! fonts — fine for a demo but not for a desktop app users live in.
//! This module owns the single-source-of-truth for the app's
//! visual identity: dark background, generous padding, balanced
//! text-style sizing, and softer widget corners. Everything else in
//! `ui/*` composes on top of this baseline.
//!
//! Kept in its own module so a future `preferences -> theme` slot
//! can swap the preset without touching the panel code.

use eframe::egui;

/// Configure the egui context with the mmcp-gui visual preset.
/// Call once from the eframe creator closure before the first
/// frame paints.
pub fn configure(ctx: &egui::Context) {
    ctx.set_visuals(build_visuals());
    ctx.set_global_style(build_style(&ctx.global_style()));
}

fn build_visuals() -> egui::Visuals {
    let mut visuals = egui::Visuals::dark();
    // Soft rather than black panel background so separators stand out.
    visuals.panel_fill = egui::Color32::from_rgb(22, 24, 28);
    visuals.window_fill = egui::Color32::from_rgb(28, 30, 34);
    visuals.extreme_bg_color = egui::Color32::from_rgb(14, 16, 20);
    visuals.faint_bg_color = egui::Color32::from_rgb(30, 32, 36);
    visuals.selection.bg_fill = egui::Color32::from_rgb(60, 95, 150);
    visuals.hyperlink_color = egui::Color32::from_rgb(120, 170, 240);
    visuals.window_stroke = egui::Stroke::new(1.0, egui::Color32::from_rgb(50, 54, 60));
    visuals
}

fn build_style(current: &egui::Style) -> egui::Style {
    use egui::{FontFamily, FontId, TextStyle};
    let mut style = current.clone();

    style.text_styles = [
        (
            TextStyle::Heading,
            FontId::new(22.0, FontFamily::Proportional),
        ),
        (TextStyle::Body, FontId::new(14.0, FontFamily::Proportional)),
        (
            TextStyle::Monospace,
            FontId::new(13.0, FontFamily::Monospace),
        ),
        (
            TextStyle::Button,
            FontId::new(14.0, FontFamily::Proportional),
        ),
        (
            TextStyle::Small,
            FontId::new(11.0, FontFamily::Proportional),
        ),
    ]
    .into();

    // Breathing room. egui defaults are tight for a professional app;
    // these values match what most modern Rust GUI apps ship.
    style.spacing.item_spacing = egui::vec2(8.0, 6.0);
    style.spacing.button_padding = egui::vec2(10.0, 6.0);
    style.spacing.menu_margin = egui::Margin::same(8);
    style.spacing.window_margin = egui::Margin::same(12);
    style.spacing.indent = 18.0;
    style.spacing.interact_size = egui::vec2(40.0, 24.0);

    // Slightly rounder widget corners for a modern feel without going
    // full pill-shaped.
    let widgets = &mut style.visuals.widgets;
    for bundle in [
        &mut widgets.noninteractive,
        &mut widgets.inactive,
        &mut widgets.hovered,
        &mut widgets.active,
        &mut widgets.open,
    ] {
        bundle.corner_radius = egui::CornerRadius::same(4);
    }

    style
}
