//! Refusal of a CLAUDE.md whose mmcp fence lacks one of its two markers.

/// A CLAUDE.md carrying one fence marker without its counterpart.
/// The managed region cannot be located, so the file is never spliced.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PartialFenceError {
    /// A begin marker with no end marker after it.
    #[error(
        "CLAUDE.md carries a partial mmcp fence: a begin marker with no end marker after it; restore the end marker or remove the begin marker"
    )]
    LoneBeginMarker,
    /// An end marker with no begin marker before it.
    #[error(
        "CLAUDE.md carries a partial mmcp fence: an end marker with no begin marker before it; restore the begin marker or remove the end marker"
    )]
    LoneEndMarker,
}
