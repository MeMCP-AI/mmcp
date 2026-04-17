//! mmcp home directory layout.
//!
//! Single source of truth for the `~/.mmcp/` directory structure.
//! Every consumer that needs repos, sessions, or the home root
//! imports from here instead of re-deriving paths from env vars.
//!
//! The home root can be overridden via the `MMCP_HOME` environment
//! variable for testing, portable installs, or custom layouts.
//! When unset, defaults to `$HOME/.mmcp` (or `$USERPROFILE/.mmcp`
//! on Windows).
//!
//! History: ported from `crates/mmcp-client/src/home.rs` during
//! the FR-020 extraction. The composition helper `init_backend`
//! that used to live here stays in `mmcp-client` until `GroupIndex`
//! follows this module into `mmcp-store`, at which point both
//! rejoin as a free function on this crate.

use std::path::{Path, PathBuf};

use anyhow::{Result, bail};

use mmcp_core::config::UserConfig;

/// Subdirectory names within the mmcp home.
const DEFAULT_MMCP_DIR: &str = ".mmcp";
const REPOS_SUBDIR: &str = "repos";
const SESSIONS_SUBDIR: &str = "sessions";
const USER_CONFIG_FILE: &str = "config.toml";

/// Resolved commit author identity.
///
/// Built by [`MmcpHome::resolve_author`] using the three-tier
/// cascade: user config -> git config (if opted in) -> fallback.
#[derive(Debug, Clone)]
pub struct ResolvedAuthor {
    pub name: String,
    pub email: String,
}

/// Resolved mmcp home directory layout.
///
/// Constructed once via [`MmcpHome::discover`] or
/// [`MmcpHome::from_root`], then passed around by reference or
/// clone. Every path mmcp consumers need is derived from this struct.
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
    /// callers that already know the root.
    #[must_use]
    pub fn from_root(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// The mmcp home root directory (e.g. `~/.mmcp`).
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Path to the bare group repositories directory.
    #[must_use]
    pub fn repos_root(&self) -> PathBuf {
        self.root.join(REPOS_SUBDIR)
    }

    /// Path to the flat session state directory.
    #[must_use]
    pub fn sessions_root(&self) -> PathBuf {
        self.root.join(SESSIONS_SUBDIR)
    }

    /// Path to the user-level config file (`~/.mmcp/config.toml`).
    #[must_use]
    pub fn user_config_path(&self) -> PathBuf {
        self.root.join(USER_CONFIG_FILE)
    }

    /// Load the user-level config. Returns `UserConfig::default()`
    /// if the file does not exist.
    pub fn load_user_config(&self) -> Result<UserConfig> {
        let path = self.user_config_path();
        if !path.exists() {
            return Ok(UserConfig::default());
        }
        let text = std::fs::read_to_string(&path)
            .map_err(|e| anyhow::anyhow!("reading {}: {e}", path.display()))?;
        UserConfig::from_toml(&text)
            .map_err(|e| anyhow::anyhow!("parsing {}: {e}", path.display()))
    }

    /// Initialize a `NativeBackend` and `GroupIndex` from this home.
    ///
    /// Canonical way to get a ready-to-use git backend and group
    /// index; every consumer (CLI, MCP, GUI, third-party) calls
    /// this once at startup.
    pub async fn init_backend(
        &self,
    ) -> anyhow::Result<(std::sync::Arc<mmcp_git::NativeBackend>, crate::groups::GroupIndex)> {
        use anyhow::Context;
        let repos_root = self.repos_root();
        let backend = std::sync::Arc::new(
            mmcp_git::NativeBackend::new(&repos_root)
                .with_context(|| format!("initializing repo root {}", repos_root.display()))?,
        );
        let groups = crate::groups::GroupIndex::build(repos_root, backend.clone())
            .await
            .with_context(|| format!("building group index at {}", self.repos_root().display()))?;
        Ok((backend, groups))
    }

    /// Resolve the commit author using the three-tier cascade:
    ///
    /// 1. `~/.mmcp/config.toml` `[author].name` / `[author].email`
    /// 2. `git config --global user.name/email` (only if `git_fallback == true`)
    /// 3. Hardcoded constants (final fallback)
    #[must_use]
    pub fn resolve_author(&self) -> ResolvedAuthor {
        let cfg = self.load_user_config().unwrap_or_default();
        let author_cfg = cfg.author.as_ref();

        let mut name: Option<String> = author_cfg.and_then(|a| a.name.clone());
        let mut email: Option<String> = author_cfg.and_then(|a| a.email.clone());

        // Tier 2: git config, only if explicitly opted in
        let git_fallback = author_cfg.and_then(|a| a.git_fallback).unwrap_or(false);
        if git_fallback && (name.is_none() || email.is_none()) {
            if name.is_none() {
                name = read_git_global("user.name");
            }
            if email.is_none() {
                email = read_git_global("user.email");
            }
        }

        // Tier 3: hardcoded fallback
        ResolvedAuthor {
            name: name.unwrap_or_else(|| mmcp_core::conventions::MMCP_AUTHOR_NAME.to_string()),
            email: email.unwrap_or_else(|| mmcp_core::conventions::MMCP_AUTHOR_EMAIL.to_string()),
        }
    }
}

/// Read a single value from git's global config via gix (no subprocess).
///
/// Keys are dotted (e.g. `user.name`). Returns `None` if the config
/// file is missing, unreadable, the key is unset, or the value is
/// empty. Shared by author resolution (Tier 2) and by the diagnose
/// command so the two never drift apart.
pub fn read_git_global(key: &str) -> Option<String> {
    let file = gix::config::File::from_globals().ok()?;
    let (section, name) = key.split_once('.')?;
    let value = file.string_by(section, None, name)?;
    let s = value.to_string();
    if s.is_empty() { None } else { Some(s) }
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
