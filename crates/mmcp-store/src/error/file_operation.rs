//! [`FileOperation`], the filesystem action tag carried by
//! [`StoreError::Io`](super::StoreError::Io).

use std::fmt;

/// Filesystem operation that failed, attached to
/// [`StoreError::Io`](super::StoreError::Io) so a caller can tell
/// "could not create the repos directory" apart from "could not read
/// the session file" by matching a field, instead of parsing the
/// message text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileOperation {
    /// Reading a file's contents (`std::fs::read_to_string`, `std::fs::read`).
    Read,
    /// Writing a file's contents (`std::fs::write`).
    Write,
    /// Creating a directory and its ancestors (`std::fs::create_dir_all`).
    CreateDir,
    /// Listing a directory's entries (`std::fs::read_dir`).
    ReadDir,
    /// Removing a file or directory tree (`std::fs::remove_file`, `std::fs::remove_dir_all`).
    Remove,
    /// Renaming or moving a path (`std::fs::rename`).
    Rename,
    /// Reading filesystem metadata (`std::fs::metadata`).
    Metadata,
}

impl fmt::Display for FileOperation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let verb = match self {
            Self::Read => "read",
            Self::Write => "write",
            Self::CreateDir => "create directory",
            Self::ReadDir => "read directory",
            Self::Remove => "remove",
            Self::Rename => "rename",
            Self::Metadata => "read metadata for",
        };
        f.write_str(verb)
    }
}
