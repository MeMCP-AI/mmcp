//! Persisted user preferences.
//!
//! Everything here survives app restarts via eframe's `Storage`
//! persistence hook. Serde-ready so adding a new setting is a single
//! field addition; the old stored file still deserializes because
//! every field defaults (either via `#[serde(default)]` or via the
//! enum's `Default` impl).
//!
//! Location on disk is platform-local and owned by eframe
//! (`~/.config/mmcp-gui/` on Linux, `%APPDATA%\mmcp-gui\` on
//! Windows). Intentionally not under `~/.mmcp/` — the memory store
//! owns that directory.

use serde::{Deserialize, Serialize};

/// Storage key under which the app persists [`UiSettings`].
pub const STORAGE_KEY: &str = "mmcp_gui_settings_v1";

/// Aggregate of every persisted preference the GUI carries.
/// Flattened intentionally so the JSON/RON encoded form stays
/// grep-friendly. New settings append as fields; never remove.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UiSettings {
    #[serde(default)]
    pub kind_display: KindDisplay,
}

/// How the memory list prefixes each slug with its kind. Radio-group
/// semantics — exactly one variant active at a time.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum KindDisplay {
    /// No prefix. Slug stands alone.
    Off,
    /// Single Unicode glyph (e.g. `§` for Rule). Default because it's
    /// the most space-efficient hint.
    #[default]
    Icon,
    /// Uppercase 3-4 letter abbreviation (e.g. `RULE`).
    Text,
    /// Icon + text, for users who want the glyph and the word.
    IconAndText,
}

impl KindDisplay {
    pub fn label(self) -> &'static str {
        match self {
            KindDisplay::Off => "Off",
            KindDisplay::Icon => "Icon only",
            KindDisplay::Text => "Text only",
            KindDisplay::IconAndText => "Icon + text",
        }
    }
}
