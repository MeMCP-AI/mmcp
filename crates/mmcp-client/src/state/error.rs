//! Error type for the client state stores.

use thiserror::Error;

/// Failures returned by [`SessionStore`](crate::state::SessionStore)
/// and [`GroupIndex`](crate::state::GroupIndex).
#[derive(Debug, Error)]
pub enum StateError {
    /// I/O failure while reading or writing a state file.
    #[error("state I/O error: {0}")]
    Io(String),

    /// TOML parse or serialize failure.
    #[error("state TOML error: {0}")]
    Toml(String),

    /// Transcript file could not be signed (underlying I/O from
    /// `mmcp-session::compute_signature`).
    #[error("transcript signature error: {0}")]
    Transcript(#[from] mmcp_session::SessionError),

    /// Git backend failure while opening a repository or reading a
    /// manifest.
    #[error("git backend error: {0}")]
    Git(#[from] mmcp_git::GitError),

    /// A group repository's manifest did not parse.
    ///
    /// NOTE: the group scanner currently logs manifest parse
    /// failures and skips the offending repo rather than failing
    /// the whole index rebuild, so this variant is unused today.
    /// Reserved for stricter flows (e.g. `mmcp import`) where a
    /// broken manifest must be fatal rather than skipped.
    #[error("manifest error: {0}")]
    #[allow(dead_code)]
    Manifest(#[from] mmcp_core::manifest::ManifestError),
}
