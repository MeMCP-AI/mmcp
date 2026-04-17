//! Unified `StoreError` type for the `mmcp-store` public surface.
//!
//! Every consumer-facing function in this crate — groups, memory,
//! sync, diagnostics — returns `Result<T, StoreError>`. Consumers
//! map this enum into their own outward shapes: the CLI maps to
//! `anyhow::Error` via `#[from]`, the MCP tools map to
//! `McpError::invalid_params` with a structured `code` payload,
//! and third-party callers match on variants directly.
//!
//! The enum is additive: new variants land when later modules are
//! ported into the store and need a code the current set doesn't
//! cover. Existing `#[from]` conversions keep `?` ergonomic at
//! call sites without requiring explicit `.map_err` plumbing.

use thiserror::Error;

/// Failures returned by the mmcp-store surface.
#[derive(Debug, Error)]
pub enum StoreError {
    /// I/O failure while reading or writing a state file.
    #[error("store I/O error: {0}")]
    Io(String),

    /// TOML parse or serialize failure.
    #[error("store TOML error: {0}")]
    Toml(String),

    /// Git backend failure while opening a repository, reading a
    /// manifest, or committing a write.
    #[error("git backend error: {0}")]
    Git(#[from] mmcp_git::GitError),

    /// A group repository's manifest did not parse.
    ///
    /// Reserved for flows that must treat a broken manifest as
    /// fatal rather than logging-and-skipping; the group-index
    /// scanner still skips these to keep an unrelated broken repo
    /// from tanking the whole mirror.
    #[error("manifest error: {0}")]
    Manifest(#[from] mmcp_core::manifest::ManifestError),

    /// Transcript signing / compaction detection failure surfaced
    /// by `mmcp-session::compute_signature`. Used by the session
    /// store's `check_transcript` path.
    #[error("transcript signature error: {0}")]
    Transcript(#[from] mmcp_session::SessionError),
}
