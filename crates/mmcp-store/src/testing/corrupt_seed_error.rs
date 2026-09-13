//! Error type for [`super::corrupt_stored_file`]'s corruption-injection helper.

use std::string::FromUtf8Error;

/// Failure while staging a corrupted memory file ahead of a test.
#[derive(Debug, thiserror::Error)]
pub enum CorruptSeedError {
    /// The underlying git read or write failed.
    #[error(transparent)]
    Git(#[from] mmcp_git::GitError),

    /// The seeded file was not valid UTF-8 before corruption, so a test marker cannot be placed.
    #[error("seeded file is not valid UTF-8 before corruption: {0}")]
    NotUtf8(#[source] FromUtf8Error),

    /// `marker` was not found in the seeded file's contents.
    #[error("corruption marker not found in the seeded file's contents")]
    MarkerNotFound,
}
