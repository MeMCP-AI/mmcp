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

/// Failure modes around driving a native Tauri dialog: no window to
/// parent it to, the result channel dropped before the callback
/// fired, or the callback returned a handle that doesn't convert to
/// a filesystem path. Shared by every command that opens a dialog
/// (`commands/archive.rs`'s export/import pickers,
/// `commands/workspace.rs`'s directory picker) so the same failure
/// mode carries the same typed variant everywhere instead of each
/// command module re-stating it as its own catch-all string.
#[derive(Debug, Error)]
pub enum GuiDialogError {
    /// No main window to parent a native dialog to.
    #[error("main window is not available")]
    NoMainWindow,

    /// The oneshot channel carrying a native dialog's result was
    /// dropped before the dialog callback fired.
    #[error("dialog channel closed before a result arrived")]
    ChannelClosed,

    /// The dialog returned a handle Tauri could not convert to a
    /// filesystem path (e.g. a non-`file://` URI).
    #[error("dialog returned an unusable path: {0}")]
    PathUnusable(String),
}

/// Failure modes specific to the archive-command Tauri layer: the
/// picked-path confinement / size-cap checks around reading an
/// archive file from disk (see `commands/archive.rs`). Dialog
/// plumbing itself is [`GuiDialogError`], not duplicated here.
#[derive(Debug, Error)]
pub enum GuiArchiveError {
    /// `value` does not name a recognized memory kind.
    #[error("unknown memory kind '{0}'")]
    UnknownKind(String),

    /// Export was requested with no groups selected.
    #[error("no groups to export")]
    NoGroupsSelected,

    /// `path` was never returned by the archive file picker, so the
    /// read is refused rather than trusting an arbitrary IPC-supplied
    /// filesystem path.
    #[error("archive path {path} was not selected through the file picker")]
    PathNotPicked { path: String },

    /// `path` is `size` bytes, exceeding the `max`-byte read cap.
    #[error("archive {path} is {size} bytes, exceeding the {max} byte limit")]
    TooLarge { path: String, size: u64, max: u64 },

    /// Reading the archive bytes from disk failed.
    #[error("reading archive {path}: {source}")]
    Read {
        path: String,
        #[source]
        source: std::io::Error,
    },
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

    /// An archive-command-layer failure: the picked-path confinement
    /// or size checks. See [`GuiArchiveError`].
    #[error("archive: {0}")]
    Archive(#[from] GuiArchiveError),

    /// A native dialog failure, shared by every command that opens
    /// one. See [`GuiDialogError`].
    #[error("dialog: {0}")]
    Dialog(#[from] GuiDialogError),

    /// Invalid UTF-8 encountered decoding process output or file
    /// contents.
    #[error("utf-8: {0}")]
    Utf8(#[from] std::str::Utf8Error),

    /// GUI-local failure with no typed source to chain: config I/O,
    /// filesystem-watcher setup, or other command-local text that has
    /// nothing more structured to extract.
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
            GuiError::Archive(e) => ("archive", Some(e.to_string())),
            GuiError::Dialog(e) => ("dialog", Some(e.to_string())),
            GuiError::Utf8(e) => ("utf8", Some(e.to_string())),
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

    #[test]
    fn archive_error_chains_to_the_real_source() {
        let source = GuiArchiveError::PathNotPicked {
            path: "/tmp/archive.tar".into(),
        };
        let err: GuiError = source.into();

        assert!(matches!(err, GuiError::Archive(_)));
        let chained = err
            .source()
            .and_then(|s| s.downcast_ref::<GuiArchiveError>())
            .expect("archive source must be preserved");
        assert!(matches!(
            chained,
            GuiArchiveError::PathNotPicked { path } if path == "/tmp/archive.tar"
        ));

        let value = serde_json::to_value(&err).unwrap();
        assert_eq!(value["kind"], "archive");
        assert_eq!(
            value["message"],
            "archive path /tmp/archive.tar was not selected through the file picker"
        );
    }

    #[test]
    fn archive_too_large_reports_size_and_limit_in_its_message() {
        let err = GuiArchiveError::TooLarge {
            path: "/tmp/big.tar".into(),
            size: 200,
            max: 100,
        };
        assert_eq!(
            err.to_string(),
            "archive /tmp/big.tar is 200 bytes, exceeding the 100 byte limit"
        );
    }
}
