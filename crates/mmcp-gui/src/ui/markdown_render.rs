//! Single source of truth for egui_commonmark rendering.
//!
//! Both the read-mode viewer and the edit-mode live preview call
//! this helper so their rendered output can never drift. Configured
//! for the app's dark theme and the "give the list some air" default
//! that the raw `CommonMarkViewer::new()` doesn't ship with.

use eframe::egui;
use egui_commonmark::{CommonMarkCache, CommonMarkViewer};

/// Syntect theme used by egui_commonmark's built-in code-block
/// highlighter. `base16-ocean.dark` ships with syntect and is the
/// closest match to the app's cool-grey palette.
const SYNTAX_THEME_DARK: &str = "base16-ocean.dark";

/// Render `body` as commonmark into `ui`, caching the parsed AST in
/// `cache` so subsequent frames are cheap. Width tracks the parent
/// `Ui`'s available space so long paragraphs and inline images wrap
/// instead of blowing out the layout.
pub fn render(ui: &mut egui::Ui, cache: &mut CommonMarkCache, body: &str) {
    // `available_width` can be f32::INFINITY during layout-probe
    // passes egui performs; clamp to a sane upper bound so
    // egui_commonmark doesn't receive `usize::MAX` as a width.
    let raw_width = ui.available_width();
    let width = if raw_width.is_finite() {
        raw_width.max(240.0) as usize
    } else {
        720
    };

    CommonMarkViewer::new()
        .default_width(Some(width))
        .max_image_width(Some(width))
        .indentation_spaces(2)
        .syntax_theme_dark(SYNTAX_THEME_DARK)
        .show(ui, cache, body);
}
