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

use super::archive::GuiArchiveError;
use super::dialog::GuiDialogError;
use super::store::GuiStoreError;

#[derive(Debug, Error)]
pub enum GuiError {
    /// A store-layer failure: memory/group read or write, import, archive, or manifest parsing.
    /// See [`GuiStoreError`] for the specific cause.
    /// Boxed: trades one indirection for a pointer-sized variant, keeping `GuiResult<T>` small.
    /// `GuiStoreError` otherwise crosses clippy's `result_large_err` threshold.
    #[error("store: {0}")]
    Store(#[from] Box<GuiStoreError>),

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

    /// `group_id` is not present in the local mirror index. Every
    /// command that resolves a group by id (list/load/create/update/
    /// delete memory) hits this before touching the group's own
    /// files, so it gets one shared, structured variant instead of
    /// being restated as a formatted string at each call site.
    #[error("group {group_id} is not in the local mirror")]
    GroupNotInMirror { group_id: String },

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

    /// A `kind` string did not match any known [`mmcp_core::memory::MemoryKind`].
    #[error("invalid memory kind: {0}")]
    InvalidMemoryKind(#[from] mmcp_core::memory::MemoryKindParseError),

    /// A `group_id` string was not a valid UUID.
    #[error("invalid group id '{value}': {source}")]
    InvalidGroupId {
        value: String,
        #[source]
        source: uuid::Error,
    },

    /// A `version` string was not valid semver.
    #[error("invalid version: {0}")]
    InvalidVersion(#[from] semver::Error),

    /// A feature `status` string did not match any known [`mmcp_core::memory::FeatureStatus`].
    #[error("invalid feature status: {0}")]
    InvalidFeatureStatus(#[from] mmcp_core::memory::FeatureStatusParseError),

    /// An issue `status` string did not match any known [`mmcp_core::memory::IssueStatus`].
    #[error("invalid issue status: {0}")]
    InvalidIssueStatus(#[from] mmcp_core::memory::IssueStatusParseError),

    /// A filesystem read, write, or directory-create failed at a known path.
    #[error("filesystem operation failed at {path}: {source}")]
    Io {
        path: std::path::PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// The process's current working directory could not be determined.
    #[error("current directory unavailable: {0}")]
    CurrentDirUnavailable(#[source] std::io::Error),

    /// `path` was expected to be a directory but is not (missing, or a regular file).
    #[error("not a directory: {}", path.display())]
    NotADirectory { path: std::path::PathBuf },

    /// A Tauri platform API failed to resolve a filesystem path (e.g. the app config directory).
    #[error("platform path resolution failed: {0}")]
    TauriPath(#[from] tauri::Error),

    /// A settings blob failed to serialize or deserialize as JSON.
    #[error("settings json: {0}")]
    SettingsJson(#[from] serde_json::Error),

    /// The mirror-root filesystem watcher failed to start or register a path.
    #[error("filesystem watcher: {0}")]
    Watcher(#[from] notify_debouncer_mini::notify::Error),

    /// GUI-local failure with no typed source to chain: command-local
    /// text that has nothing more structured to extract.
    #[error("{0}")]
    Other(String),
}

impl From<mmcp_store::StoreError> for GuiError {
    fn from(e: mmcp_store::StoreError) -> Self {
        GuiError::Store(Box::new(e.into()))
    }
}

impl From<mmcp_store::ImportError> for GuiError {
    fn from(e: mmcp_store::ImportError) -> Self {
        GuiError::Store(Box::new(e.into()))
    }
}

impl From<mmcp_store::ArchiveError> for GuiError {
    fn from(e: mmcp_store::ArchiveError) -> Self {
        GuiError::Store(Box::new(e.into()))
    }
}

impl From<mmcp_core::memory::MemoryParseError> for GuiError {
    fn from(e: mmcp_core::memory::MemoryParseError) -> Self {
        GuiError::Store(Box::new(e.into()))
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
            GuiError::GroupNotInMirror { .. } => ("group_not_in_mirror", Some(self.to_string())),
            GuiError::Archive(e) => ("archive", Some(e.to_string())),
            GuiError::Dialog(e) => ("dialog", Some(e.to_string())),
            GuiError::Utf8(e) => ("utf8", Some(e.to_string())),
            GuiError::InvalidMemoryKind(e) => ("invalid_memory_kind", Some(e.to_string())),
            GuiError::InvalidGroupId { .. } => ("invalid_group_id", Some(self.to_string())),
            GuiError::InvalidVersion(e) => ("invalid_version", Some(e.to_string())),
            GuiError::InvalidFeatureStatus(e) => ("invalid_feature_status", Some(e.to_string())),
            GuiError::InvalidIssueStatus(e) => ("invalid_issue_status", Some(e.to_string())),
            GuiError::Io { .. } => ("io", Some(self.to_string())),
            GuiError::CurrentDirUnavailable(e) => ("current_dir_unavailable", Some(e.to_string())),
            GuiError::NotADirectory { .. } => ("not_a_directory", Some(self.to_string())),
            GuiError::TauriPath(e) => ("tauri_path", Some(e.to_string())),
            GuiError::SettingsJson(e) => ("settings_json", Some(e.to_string())),
            GuiError::Watcher(e) => ("watcher", Some(e.to_string())),
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
        // `Box<GuiStoreError>` delegates `source()` to the inner error.
        // One extra hop reaches `mmcp_store::StoreError`, with no downcast of the box.
        let level1 = err.source().expect("gui-store source must be preserved");
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

    /// Asserts `GroupNotInMirror` carries its own wire `kind`, distinct from the `Other` catch-all,
    /// and embeds the offending group id in the message.
    #[test]
    fn group_not_in_mirror_has_its_own_wire_kind_and_names_the_group() {
        let err = GuiError::GroupNotInMirror {
            group_id: "019d955d-4cce-77f2-a0b3-0b79ed394612".to_string(),
        };
        let value = serde_json::to_value(&err).unwrap();
        assert_eq!(value["kind"], "group_not_in_mirror");
        assert_eq!(
            value["message"],
            "group 019d955d-4cce-77f2-a0b3-0b79ed394612 is not in the local mirror"
        );
    }

    /// Before the typed variants below existed, every classifiable
    /// parse/IO failure serialized as the `other` catch-all with no
    /// stable code to branch on. Each of these must now report its
    /// own distinct `kind`, and must chain to the real source error.
    #[test]
    fn classifiable_causes_no_longer_serialize_as_the_other_catch_all() {
        let invalid_kind: GuiError = "not-a-kind"
            .parse::<mmcp_core::memory::MemoryKind>()
            .unwrap_err()
            .into();
        assert_eq!(
            serde_json::to_value(&invalid_kind).unwrap()["kind"],
            "invalid_memory_kind"
        );

        let invalid_group_id = GuiError::InvalidGroupId {
            value: "not-a-uuid".to_string(),
            source: uuid::Uuid::parse_str("not-a-uuid").unwrap_err(),
        };
        assert_eq!(
            serde_json::to_value(&invalid_group_id).unwrap()["kind"],
            "invalid_group_id"
        );
        let chained = invalid_group_id
            .source()
            .and_then(|s| s.downcast_ref::<uuid::Error>());
        assert!(chained.is_some(), "uuid source must be preserved");

        let invalid_version: GuiError = semver::Version::parse("not-a-version").unwrap_err().into();
        assert_eq!(
            serde_json::to_value(&invalid_version).unwrap()["kind"],
            "invalid_version"
        );

        let not_a_directory = GuiError::NotADirectory {
            path: std::path::PathBuf::from("/no/such/dir"),
        };
        assert_eq!(
            serde_json::to_value(&not_a_directory).unwrap()["kind"],
            "not_a_directory"
        );
    }
}
