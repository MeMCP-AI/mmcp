//! Unified GUI error type.
//!
//! Every command surfaces this type on failure. The frontend
//! discriminates on `kind` (toasting `message` for most failures;
//! branching only where UI logic needs to, e.g. "not configured" vs
//! "connection refused"). The wire shape (`{"kind": ..., "message":
//! ...}`) is produced by the hand-written `Serialize` impl below so
//! that contract stays stable while the Rust-side variants carry
//! real, structured source chains instead of pre-formatted strings.

use serde::ser::SerializeStruct;
use serde::{Serialize, Serializer};
use thiserror::Error;

/// Store-layer failures surfaced to the GUI.
///
/// Groups every distinct store-adjacent source type the GUI can
/// receive (the store's own I/O/manifest/git errors, plus import,
/// archive, and memory-parse failures) behind one facade so
/// `GuiError::Store` keeps a single field while each cause stays its
/// own variant with its own source chain.
#[derive(Debug, Error)]
pub enum GuiStoreError {
    #[error("{0}")]
    Store(#[from] mmcp_store::StoreError),

    #[error("{0}")]
    Import(#[from] mmcp_store::ImportError),

    #[error("{0}")]
    Archive(#[from] mmcp_store::ArchiveError),

    #[error("{0}")]
    MemoryParse(#[from] mmcp_core::memory::MemoryParseError),
}

#[derive(Debug, Error)]
pub enum GuiError {
    /// A store-layer failure: memory/group read or write, import,
    /// archive, or manifest parsing. See [`GuiStoreError`] for the
    /// specific cause.
    #[error("store: {0}")]
    Store(#[from] GuiStoreError),

    /// A git-backend failure while opening a repository, reading a
    /// manifest, or committing a write.
    #[error("git: {0}")]
    Git(#[from] mmcp_git::GitError),

    /// A sync-engine failure while pulling or pushing.
    #[error("sync: {0}")]
    Sync(#[from] mmcp_sync::SyncError),

    /// No sync bundle is configured for this session.
    #[error("sync-not-configured")]
    SyncNotConfigured,

    /// GUI-local failure with no typed source to chain: dialog
    /// plumbing, config I/O, or other command-local text that has
    /// nothing structured to extract.
    #[error("{0}")]
    Other(String),
}

impl From<mmcp_store::StoreError> for GuiError {
    fn from(e: mmcp_store::StoreError) -> Self {
        GuiError::Store(e.into())
    }
}

impl From<mmcp_store::ImportError> for GuiError {
    fn from(e: mmcp_store::ImportError) -> Self {
        GuiError::Store(e.into())
    }
}

impl From<mmcp_store::ArchiveError> for GuiError {
    fn from(e: mmcp_store::ArchiveError) -> Self {
        GuiError::Store(e.into())
    }
}

impl From<mmcp_core::memory::MemoryParseError> for GuiError {
    fn from(e: mmcp_core::memory::MemoryParseError) -> Self {
        GuiError::Store(e.into())
    }
}

impl From<anyhow::Error> for GuiError {
    fn from(e: anyhow::Error) -> Self {
        GuiError::Other(e.to_string())
    }
}

impl From<std::str::Utf8Error> for GuiError {
    fn from(e: std::str::Utf8Error) -> Self {
        GuiError::Other(format!("utf-8: {e}"))
    }
}

// Hand-written rather than derived: the frontend's `GuiErrorPayload`
// TS type is a flat `{ kind, message? }` shape. A derived adjacently
// tagged `Serialize` would nest the wrapped source error's own
// fields under `message` once the payload is a real source type
// instead of a bare `String`, breaking that contract. This impl
// keeps the wire format identical while the enum itself carries
// structured, source-chained variants.
impl Serialize for GuiError {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let (kind, message) = match self {
            GuiError::Store(e) => ("store", Some(e.to_string())),
            GuiError::Git(e) => ("git", Some(e.to_string())),
            GuiError::Sync(e) => ("sync", Some(e.to_string())),
            GuiError::SyncNotConfigured => ("sync_not_configured", None),
            GuiError::Other(msg) => ("other", Some(msg.clone())),
        };
        let mut state = serializer.serialize_struct("GuiError", 2)?;
        state.serialize_field("kind", kind)?;
        state.serialize_field("message", &message)?;
        state.end()
    }
}

pub type GuiResult<T> = std::result::Result<T, GuiError>;

#[cfg(test)]
mod tests {
    use std::error::Error as _;

    use super::*;

    #[test]
    fn git_error_chains_to_the_real_source() {
        let source = mmcp_git::GitError::RepoNotFound("repo-x".into());
        let err: GuiError = source.into();

        assert!(matches!(err, GuiError::Git(_)));
        let chained = err
            .source()
            .and_then(|s| s.downcast_ref::<mmcp_git::GitError>())
            .expect("git source must be preserved");
        assert!(matches!(chained, mmcp_git::GitError::RepoNotFound(name) if name == "repo-x"));
    }

    #[test]
    fn store_error_chains_through_gui_store_error_to_the_real_source() {
        let source = mmcp_store::StoreError::Git(mmcp_git::GitError::RepoNotFound("repo-y".into()));
        let err: GuiError = source.into();

        assert!(matches!(err, GuiError::Store(_)));
        let level1 = err
            .source()
            .and_then(|s| s.downcast_ref::<GuiStoreError>())
            .expect("gui-store source must be preserved");
        let level2 = level1
            .source()
            .and_then(|s| s.downcast_ref::<mmcp_store::StoreError>())
            .expect("store source must be preserved");
        let level3 = level2
            .source()
            .and_then(|s| s.downcast_ref::<mmcp_git::GitError>())
            .expect("git source must be preserved at the bottom of the chain");
        assert!(matches!(level3, mmcp_git::GitError::RepoNotFound(name) if name == "repo-y"));
    }

    #[test]
    fn sync_error_chains_to_the_real_source() {
        let source = mmcp_sync::SyncError::NotFound("edit-z".into());
        let err: GuiError = source.into();

        assert!(matches!(err, GuiError::Sync(_)));
        let chained = err
            .source()
            .and_then(|s| s.downcast_ref::<mmcp_sync::SyncError>())
            .expect("sync source must be preserved");
        assert!(matches!(chained, mmcp_sync::SyncError::NotFound(id) if id == "edit-z"));
    }

    #[test]
    fn wire_shape_stays_a_flat_kind_message_pair() {
        let err: GuiError = mmcp_git::GitError::RepoNotFound("repo-x".into()).into();
        let value = serde_json::to_value(&err).unwrap();
        assert_eq!(value["kind"], "git");
        assert_eq!(value["message"], "repository not found: repo-x");
    }

    #[test]
    fn unit_variant_serializes_with_a_null_message() {
        let value = serde_json::to_value(GuiError::SyncNotConfigured).unwrap();
        assert_eq!(value["kind"], "sync_not_configured");
        assert!(value["message"].is_null());
    }
}
