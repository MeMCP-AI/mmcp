//! Facts about the git checkout a file lives in.
//!
//! Serves a personal file that belongs next to a project but never in its history:
//! where the file sits inside a linked worktree, whether the repository already ignores it,
//! and how it gets excluded through the user's global excludes file.
//! Every operation goes through `gix`, never a `git` subprocess.

mod exclude_path;
mod exclusion;
mod global_excludes;
#[cfg(any(test, feature = "testing"))]
mod linked_worktree_fixture;
mod main_checkout;
mod open_checkout;
mod path_exclusion;

pub use exclude_path::exclude_path_globally;
pub use exclusion::Exclusion;
pub use global_excludes::global_excludes_file;
#[cfg(any(test, feature = "testing"))]
pub use linked_worktree_fixture::LinkedWorktreeFixture;
pub use main_checkout::main_checkout_counterpart;
pub use path_exclusion::is_path_excluded;
