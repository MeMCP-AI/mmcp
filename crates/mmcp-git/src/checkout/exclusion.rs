//! [`Exclusion`], what excluding a path from git did.

use std::path::PathBuf;

/// Outcome of [`exclude_path_globally`](super::exclude_path_globally).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Exclusion {
    /// The path is not inside a git repository, so nothing needed excluding.
    NotInRepository,
    /// The repository's ignore rules already exclude the path.
    AlreadyExcluded,
    /// The repository tracks the path, so no ignore rule keeps it out of git.
    Tracked,
    /// The pattern was appended to the global excludes file `file`.
    Appended {
        /// The global excludes file that received the pattern.
        file: PathBuf,
    },
}
