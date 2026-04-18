//! Visual vocabulary for `MemoryKind`.
//!
//! Previous revisions used Unicode symbols (§ ◐ ≡ → ▢ ★) that didn't
//! render reliably on every platform. This version expresses kind
//! purely through short Latin labels plus a distinctive accent
//! colour per kind — letters always render, colour carries the
//! at-a-glance hint. Every site that shows a kind (memory list
//! prefix, settings-panel preview, viewer pill row) goes through
//! these helpers so the colour assignment and label set stay
//! centralised.

use eframe::egui;
use mmcp_core::memory::MemoryKind;

use crate::state::settings::KindDisplay;
use crate::ui::tag_pill;

/// Two-char lowercase abbreviation. Compact enough for the
/// `Icon`-mode pill in the memory list while still reading as a
/// name rather than a glyph.
pub const fn kind_short(kind: MemoryKind) -> &'static str {
    match kind {
        MemoryKind::Rule => "ru",
        MemoryKind::Snapshot => "sn",
        MemoryKind::Log => "lg",
        MemoryKind::Reference => "rf",
        MemoryKind::Scratch => "sc",
        MemoryKind::Feature => "ft",
    }
}

/// Four-char uppercase abbreviation for Text / IconAndText modes.
pub const fn kind_long(kind: MemoryKind) -> &'static str {
    match kind {
        MemoryKind::Rule => "RULE",
        MemoryKind::Snapshot => "SNAP",
        MemoryKind::Log => "LOG",
        MemoryKind::Reference => "REF",
        MemoryKind::Scratch => "SCR",
        MemoryKind::Feature => "FEAT",
    }
}

/// Accent colour per kind. Picked for contrast against the dark
/// panel background; each shade reads distinctly when multiple
/// kinds sit next to each other in a list.
pub const fn kind_color(kind: MemoryKind) -> egui::Color32 {
    match kind {
        // Authority blue
        MemoryKind::Rule => egui::Color32::from_rgb(96, 160, 240),
        // Capture purple
        MemoryKind::Snapshot => egui::Color32::from_rgb(170, 130, 230),
        // Journal amber
        MemoryKind::Log => egui::Color32::from_rgb(230, 180, 90),
        // Link teal
        MemoryKind::Reference => egui::Color32::from_rgb(90, 200, 190),
        // Neutral grey
        MemoryKind::Scratch => egui::Color32::from_rgb(150, 160, 170),
        // Feature orange
        MemoryKind::Feature => egui::Color32::from_rgb(240, 150, 90),
    }
}

/// Render the kind prefix inline per the user's display mode.
/// No-op for `Off`. The memory-list panel wraps this in a
/// horizontal row so the pill sits left of the slug.
pub fn render_prefix(ui: &mut egui::Ui, mode: KindDisplay, kind: MemoryKind) {
    let accent = kind_color(kind);
    match mode {
        KindDisplay::Off => {}
        KindDisplay::Icon => {
            tag_pill::kind_colored(ui, kind_short(kind), accent);
        }
        KindDisplay::Text => {
            let v = ui.visuals();
            tag_pill::show(
                ui,
                kind_long(kind),
                v.widgets.inactive.weak_bg_fill,
                v.widgets.inactive.fg_stroke.color,
            );
        }
        KindDisplay::IconAndText => {
            tag_pill::kind_colored(ui, kind_long(kind), accent);
        }
    }
}

/// Render a neutral placeholder prefix the same width as a real
/// one, so slug text doesn't jump sideways as per-memory
/// frontmatter loads land progressively.
pub fn render_placeholder(ui: &mut egui::Ui, mode: KindDisplay) {
    let v = ui.visuals();
    let fill = v.widgets.noninteractive.weak_bg_fill;
    let fg = v.widgets.noninteractive.fg_stroke.color;
    match mode {
        KindDisplay::Off => {}
        KindDisplay::Icon => {
            tag_pill::show(ui, "··", fill, fg);
        }
        KindDisplay::Text | KindDisplay::IconAndText => {
            tag_pill::show(ui, "····", fill, fg);
        }
    }
}
