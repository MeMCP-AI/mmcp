//! Typed errors for the archive export / import paths.

use uuid::Uuid;

/// Failure modes of the archive export and import paths.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ArchiveError {
    /// Filesystem or tar-stream I/O failed.
    #[error("archive I/O failed")]
    Io(#[from] std::io::Error),

    /// A git read against a source group failed during export, or a
    /// write against a target group failed during import.
    #[error("git backend error")]
    Git(#[from] mmcp_git::GitError),

    /// Replaying a memory into the store failed during import.
    #[error("importing a memory failed")]
    Import(#[from] crate::memory::ImportError),

    /// Rescanning the group index after creating a group failed.
    #[error("store index error")]
    Store(#[from] crate::error::StoreError),

    /// Rendering the archive table of contents to TOML failed.
    #[error("serializing the archive manifest failed")]
    ManifestSerialize(#[from] toml::ser::Error),

    /// Parsing the archive table of contents from TOML failed.
    #[error("parsing the archive manifest failed")]
    ManifestParse(#[from] toml::de::Error),

    /// A group's `.mmcp.toml` inside the archive did not parse.
    #[error("parsing the group manifest for group {group_id} failed")]
    GroupManifestParse {
        group_id: Uuid,
        #[source]
        source: mmcp_core::manifest::ManifestError,
    },

    /// The archive has no root `archive.toml` table of contents.
    #[error("archive is missing its `archive.toml` table of contents")]
    MissingManifest,

    /// The archive lists a group but carries no `.mmcp.toml` for it.
    #[error("archive lists group {group_id} but contains no manifest for it")]
    GroupManifestMissing { group_id: Uuid },

    /// The archive's layout version is newer than this build reads.
    #[error("unsupported archive format version {found}; this build reads up to {supported}")]
    UnsupportedFormatVersion { found: u32, supported: u32 },

    /// A target group named for import is protected and the caller
    /// did not opt into writing protected groups.
    #[error("target group {slug} ({group_id}) is protected; confirm the write before importing")]
    ProtectedGroup { group_id: Uuid, slug: String },

    /// `--into` named a target group that is not in the local mirror.
    #[error("target group `{0}` for import was not found in the local mirror")]
    IntoGroupNotFound(String),

    /// A tar entry violated the archive layout contract.
    #[error("malformed archive: {detail}")]
    Malformed { detail: String },

    /// A snapshot-only option was set for a history restore, which is
    /// whole-repo and all-or-nothing per group.
    #[error("option `{option}` applies to snapshot import only, not a history restore")]
    SnapshotOnlyOption { option: &'static str },

    /// A text entry (manifest or memory) was not valid UTF-8.
    #[error("archive entry `{path}` is not valid UTF-8")]
    NotUtf8 {
        path: String,
        #[source]
        source: std::str::Utf8Error,
    },
}
