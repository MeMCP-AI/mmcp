//! Warnings attached to memory retrieval responses.

use serde::{Deserialize, Serialize};

/// Classification of a [`Warning`] so clients can render them
/// consistently.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WarningKind {
    /// The memory's content is intrinsically time-sensitive (kind =
    /// snapshot). Always attached for snapshot memories.
    StaleByKind,

    /// The session has not read this memory yet.
    FirstReadThisSession,

    /// The memory's current version differs from what this session
    /// last saw.
    ChangedSinceLastRead,

    /// A compaction event happened after the last read and the
    /// session must re-verify mandatory memories.
    PostCompaction,

    /// A catch-all for future warning kinds. Unknown kinds received
    /// by an older client should be passed through to the user.
    #[serde(other)]
    Other,
}

/// A single warning message emitted with a retrieval response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Warning {
    /// Machine-readable classification.
    pub kind: WarningKind,

    /// Human-readable explanation rendered into the response.
    pub message: String,
}

impl Warning {
    #[must_use]
    pub fn new(kind: WarningKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
}
