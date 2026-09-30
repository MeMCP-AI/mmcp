//! Test fixture: a main checkout with one linked worktree.

use std::path::PathBuf;

use super::open_checkout::real_path;
use crate::GitError;

/// A main checkout with one linked worktree registered the way `git worktree add` registers it.
/// The scratch directory holding both is removed on drop.
pub struct LinkedWorktreeFixture {
    _scratch: tempfile::TempDir,
    /// Root of the main checkout.
    pub main: PathBuf,
    /// Root of the linked worktree.
    pub linked: PathBuf,
}

impl LinkedWorktreeFixture {
    /// Create the main checkout and its linked worktree under a fresh scratch directory.
    ///
    /// # Errors
    /// [`GitError::Io`] when a directory or file cannot be created, [`GitError::ResolvePath`] when the scratch path cannot be resolved, [`GitError::OpenRepo`] when the main checkout cannot be initialised.
    pub fn new() -> Result<Self, GitError> {
        let scratch = tempfile::TempDir::new()?;
        let base = real_path(scratch.path())?;
        let main = base.join("main");
        std::fs::create_dir_all(&main)?;
        gix::init(&main).map_err(|error| GitError::OpenRepo {
            path: main.display().to_string(),
            source: Box::new(error),
        })?;
        let linked = base.join("linked");
        std::fs::create_dir_all(&linked)?;
        let registration = main.join(".git").join("worktrees").join("linked");
        std::fs::create_dir_all(&registration)?;
        std::fs::write(registration.join("HEAD"), "ref: refs/heads/main\n")?;
        std::fs::write(registration.join("commondir"), "../..\n")?;
        std::fs::write(
            registration.join("gitdir"),
            format!("{}\n", linked.join(".git").display()),
        )?;
        std::fs::write(
            linked.join(".git"),
            format!("gitdir: {}\n", registration.display()),
        )?;
        Ok(Self {
            _scratch: scratch,
            main,
            linked,
        })
    }
}
