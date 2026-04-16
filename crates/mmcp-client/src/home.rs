//! mmcp home directory layout.
//!
//! Single source of truth for the `~/.mmcp/` directory structure.
//! Every command that needs repos, sessions, or the home root
//! imports from here instead of re-deriving paths from env vars.
//!
//! The home root can be overridden via the `MMCP_HOME` environment
//! variable for testing, portable installs, or custom layouts.
//! When unset, defaults to `$HOME/.mmcp` (or `$USERPROFILE/.mmcp`
//! on Windows).

use std::path::{Path, PathBuf};

use anyhow::{Result, bail};

/// Subdirectory names within the mmcp home.
const DEFAULT_MMCP_DIR: &str = ".mmcp";
const REPOS_SUBDIR: &str = "repos";
const SESSIONS_SUBDIR: &str = "sessions";

/// Resolved mmcp home directory layout.
///
/// Constructed once via [`MmcpHome::discover`] or
/// [`MmcpHome::from_root`], then passed around by reference or
/// clone. Every path the client needs is derived from this struct.
#[derive(Debug, Clone)]
pub struct MmcpHome {
    root: PathBuf,
}

impl MmcpHome {
    /// Discover the mmcp home from the environment.
    ///
    /// Resolution order:
    /// 1. `MMCP_HOME` env var (explicit override)
    /// 2. `$HOME/.mmcp` (Unix / Git Bash on Windows)
    /// 3. `$USERPROFILE/.mmcp` (native Windows)
    pub fn discover() -> Result<Self> {
        if let Ok(explicit) = std::env::var("MMCP_HOME") {
            return Ok(Self {
                root: PathBuf::from(explicit),
            });
        }
        let home = resolve_user_home()?;
        Ok(Self {
            root: home.join(DEFAULT_MMCP_DIR),
        })
    }

    /// Build from an explicit root path. Used by tests and by
    /// callers that already know the root (e.g. `initialize_at`).
    pub fn from_root(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// The mmcp home root directory (e.g. `~/.mmcp`).
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Path to the bare group repositories directory.
    pub fn repos_root(&self) -> PathBuf {
        self.root.join(REPOS_SUBDIR)
    }

    /// Path to the flat session state directory.
    pub fn sessions_root(&self) -> PathBuf {
        self.root.join(SESSIONS_SUBDIR)
    }
}

/// Resolve the user's home directory from environment variables.
fn resolve_user_home() -> Result<PathBuf> {
    if let Ok(home) = std::env::var("HOME") {
        return Ok(PathBuf::from(home));
    }
    if let Ok(profile) = std::env::var("USERPROFILE") {
        return Ok(PathBuf::from(profile));
    }
    bail!("cannot determine home directory: set MMCP_HOME, HOME, or USERPROFILE")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_root_derives_subdirs() {
        let home = MmcpHome::from_root("/tmp/test-mmcp");
        assert_eq!(home.root(), Path::new("/tmp/test-mmcp"));
        assert_eq!(home.repos_root(), PathBuf::from("/tmp/test-mmcp/repos"));
        assert_eq!(
            home.sessions_root(),
            PathBuf::from("/tmp/test-mmcp/sessions")
        );
    }
}
