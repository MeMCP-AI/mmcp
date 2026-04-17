//! Centralised `MemoryKind` visual vocabulary.
//!
//! Every site that renders a kind prefix / badge consults this module
//! so the glyph set stays consistent across the memory-list prefix,
//! the settings-panel preview, and future UIs that want to show kind
//! at a glance. Glyphs are regular Unicode (no emoji font dependency)
//! so they render reliably on Windows, Linux, and macOS without
//! requiring a colour-emoji font.

use mmcp_core::memory::MemoryKind;

use crate::state::settings::KindDisplay;

/// Single-character Unicode glyph per kind.
pub const fn kind_icon(kind: MemoryKind) -> &'static str {
    match kind {
        MemoryKind::Rule => "§",
        MemoryKind::Snapshot => "◐",
        MemoryKind::Log => "≡",
        MemoryKind::Reference => "→",
        MemoryKind::Scratch => "▢",
        MemoryKind::Fr => "★",
    }
}

/// Fixed-width 4-char text abbreviation per kind. Uppercased for
/// scannability; padded so every label occupies the same slot when
/// the settings mode is `Text` or `IconAndText`.
pub const fn kind_text(kind: MemoryKind) -> &'static str {
    match kind {
        MemoryKind::Rule => "RULE",
        MemoryKind::Snapshot => "SNAP",
        MemoryKind::Log => "LOG ",
        MemoryKind::Reference => "REF ",
        MemoryKind::Scratch => "SCR ",
        MemoryKind::Fr => "FR  ",
    }
}

/// Rendered prefix string for `kind` under the given display mode.
/// Returns an empty string when the mode is `Off`. The trailing space
/// is baked in so the caller concatenates without having to pad.
pub fn prefix_for(mode: KindDisplay, kind: MemoryKind) -> String {
    match mode {
        KindDisplay::Off => String::new(),
        KindDisplay::Icon => format!("{}  ", kind_icon(kind)),
        KindDisplay::Text => format!("{}  ", kind_text(kind)),
        KindDisplay::IconAndText => {
            format!("{} {}  ", kind_icon(kind), kind_text(kind))
        }
    }
}

/// Placeholder used while a memory's kind is not yet known
/// (frontmatter still loading). Shaped so it occupies the same slot
/// as a real prefix under each display mode, avoiding the "slug
/// jumps left then right" flicker as loads complete.
pub fn placeholder_prefix(mode: KindDisplay) -> &'static str {
    match mode {
        KindDisplay::Off => "",
        KindDisplay::Icon => "·  ",
        KindDisplay::Text => "...   ",
        KindDisplay::IconAndText => "· ...   ",
    }
}
