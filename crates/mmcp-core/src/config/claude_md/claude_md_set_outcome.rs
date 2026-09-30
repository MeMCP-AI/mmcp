//! [`ClaudeMdSetOutcome`], what a write to a `claude_md` table did.

/// Outcome of setting or removing the suggestion at one configuration level.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClaudeMdSetOutcome {
    /// Whether the stored value differs from before the write.
    pub changed: bool,
    /// Whether the write replaced an invalid table or value.
    pub replaced_invalid: bool,
}
