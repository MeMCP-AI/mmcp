//! Standard notes channel for MCP tool responses (FR-45).
//!
//! Every MCP tool response envelope carries an optional
//! `notes: Vec<Note>` field. Any code path inside the server
//! (memory loader, manifest reader, ref resolver, sync engine,
//! frontmatter parser) accumulates notes into a per-request
//! queue; the tool entry point drains that queue into the
//! response just before serialisation.
//!
//! Callers that do not care ignore the field (empty / absent in
//! the common case). Callers that do care get one uniform place
//! to look, regardless of which tool they invoked.
//!
//! Note codes draw from a documented, stable vocabulary. Adding
//! a new code is a deliberate contract change. Clients can match
//! against specific codes (e.g. a GUI rendering a special icon
//! for `dangling_ref`).

use serde::{Deserialize, Serialize};

/// Severity level for a [`Note`]. Callers can render info as a
/// plain tip, warn as a yellow prefix, error as a red prefix (but
/// error here is still a non-fatal signal — failed calls return
/// a proper `McpError` / `ProtoError` instead).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NoteLevel {
    /// Informational signal; the call succeeded without anomaly,
    /// the note is a hint or diagnostic context.
    Info,
    /// Something unusual happened that did not fail the call but
    /// the caller should surface it (dangling ref, stale cache,
    /// deprecated argument form).
    Warn,
    /// A non-fatal error surfaced during the operation (a single
    /// sub-item failed but the call as a whole succeeded). Hard
    /// failures still return through the error channel.
    Error,
}

/// One entry in the notes channel.
///
/// `code` is a stable machine-readable identifier drawn from a
/// documented vocabulary (see the FR-45 body for the initial set
/// and the ongoing changelog). `context` carries structured
/// payload — the target UUID, file path, remote URL, whatever
/// the specific note code promises to ship.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Note {
    /// Severity. Serialised as lowercase: `info` / `warn` / `error`.
    pub level: NoteLevel,

    /// Stable machine-readable code like `dangling_ref`,
    /// `id_mismatch_accepted`, `stale_by_kind`. Callers match
    /// against specific codes to render them distinctly.
    pub code: String,

    /// Human-readable one-line explanation. Suitable for direct
    /// display in a CLI tail or a GUI tooltip.
    pub message: String,

    /// Optional structured payload. Shape is code-specific and
    /// documented alongside each code.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<serde_json::Value>,
}

impl Note {
    /// Build a note with no structured context.
    #[must_use]
    pub fn new(level: NoteLevel, code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            level,
            code: code.into(),
            message: message.into(),
            context: None,
        }
    }

    /// Attach a structured context payload. Shape is note-code
    /// specific; see the code vocabulary docs.
    #[must_use]
    pub fn with_context(mut self, context: serde_json::Value) -> Self {
        self.context = Some(context);
        self
    }

    /// Convenience: `Info` level.
    #[must_use]
    pub fn info(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self::new(NoteLevel::Info, code, message)
    }

    /// Convenience: `Warn` level.
    #[must_use]
    pub fn warn(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self::new(NoteLevel::Warn, code, message)
    }

    /// Convenience: `Error` level.
    #[must_use]
    pub fn error(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self::new(NoteLevel::Error, code, message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn note_round_trips_through_serde_json_with_context() {
        let n = Note::warn("dangling_ref", "ref target does not resolve")
            .with_context(json!({ "target": "019d9d93-7cbc-7fd2-afb4-ba61328ff984" }));
        let rendered = serde_json::to_string(&n).expect("render");
        let parsed: Note = serde_json::from_str(&rendered).expect("parse");
        assert_eq!(parsed, n);
    }

    #[test]
    fn note_without_context_omits_the_field_on_serialize() {
        let n = Note::info("first_read_this_session", "session has not read this yet");
        let rendered = serde_json::to_string(&n).expect("render");
        assert!(
            !rendered.contains("context"),
            "context must be skipped when absent; rendered: {rendered}",
        );
        let parsed: Note = serde_json::from_str(&rendered).expect("parse");
        assert_eq!(parsed, n);
    }

    #[test]
    fn note_level_serialises_lowercase_snake_case() {
        for (level, expected) in [
            (NoteLevel::Info, "\"info\""),
            (NoteLevel::Warn, "\"warn\""),
            (NoteLevel::Error, "\"error\""),
        ] {
            let rendered = serde_json::to_string(&level).expect("render");
            assert_eq!(rendered, expected);
        }
    }

    #[test]
    fn note_level_parses_back_from_snake_case() {
        let parsed: NoteLevel = serde_json::from_str("\"warn\"").expect("parse");
        assert_eq!(parsed, NoteLevel::Warn);
    }
}
