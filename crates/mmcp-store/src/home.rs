//! mmcp home directory layout.
//!
//! Single source of truth for the `~/.mmcp/` directory structure.
//! Every consumer that needs repos, sessions, or the home root,
//! imports from here instead of re-deriving paths from env vars.
//!
//! The home root can be overridden via the `MMCP_HOME` environment variable,
//! for testing, portable installs, or custom layouts.
//! When unset, defaults to `$HOME/.mmcp` (or `$USERPROFILE/.mmcp` on Windows).
//!
//! The composition helper `init_backend` lives in `mmcp-client` until `GroupIndex`
//! follows this module into `mmcp-store`; then both rejoin as a free function here.

use std::path::{Path, PathBuf};

use mmcp_core::config::UserConfig;

use crate::error::{FileOperation, StoreError};

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
/// Constructed once via [`MmcpHome::discover`] or [`MmcpHome::from_root`], then passed around by reference or clone.
/// Every path mmcp consumers need is derived from this struct.
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
    pub fn discover() -> Result<Self, StoreError> {
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

    /// Build from an explicit root path.
    /// Used by tests and by callers that already know the root.
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

    /// Load the user-level config.
    /// Returns `UserConfig::default()` if the file does not exist.
    pub fn load_user_config(&self) -> Result<UserConfig, StoreError> {
        let path = self.user_config_path();
        if !path.exists() {
            return Ok(UserConfig::default());
        }
        let text = std::fs::read_to_string(&path)
            .map_err(|source| StoreError::io(path.clone(), FileOperation::Read, source))?;
        UserConfig::from_toml(&text).map_err(|source| StoreError::TomlParse { path, source })
    }

    /// Persist the user-level config.
    /// Creates the home directory if it does not yet exist,
    /// so callers can write the first config without a separate `init` step.
    pub fn save_user_config(&self, cfg: &UserConfig) -> Result<(), StoreError> {
        let path = self.user_config_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|source| {
                StoreError::io(parent.to_path_buf(), FileOperation::CreateDir, source)
            })?;
        }
        let text = cfg.to_toml().map_err(|source| StoreError::TomlSerialize {
            path: path.clone(),
            source,
        })?;
        std::fs::write(&path, text)
            .map_err(|source| StoreError::io(path, FileOperation::Write, source))?;
        Ok(())
    }

    /// Initialize a `NativeBackend` and `GroupIndex` from this home.
    ///
    /// Canonical way to get a ready-to-use git backend and group
    /// index; every consumer (CLI, MCP, GUI, third-party) calls
    /// this once at startup.
    pub async fn init_backend(
        &self,
    ) -> Result<
        (
            std::sync::Arc<mmcp_git::NativeBackend>,
            crate::groups::GroupIndex,
        ),
        StoreError,
    > {
        let repos_root = self.repos_root();
        let backend = std::sync::Arc::new(mmcp_git::NativeBackend::new(&repos_root)?);
        let groups = crate::groups::GroupIndex::build(repos_root, backend.clone()).await?;
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
/// Keys are dotted (e.g. `user.name`).
/// Returns `None` if the config file is missing, unreadable, the key is unset, or the value is empty.
/// Shared by author resolution (Tier 2) and by the diagnose command so the two never drift apart.
pub fn read_git_global(key: &str) -> Option<String> {
    let file = gix::config::File::from_globals().ok()?;
    let (section, name) = key.split_once('.')?;
    let value = file.string_by(section, None, name)?;
    let s = value.to_string();
    if s.is_empty() { None } else { Some(s) }
}

/// Resolve the user's home directory from environment variables.
fn resolve_user_home() -> Result<PathBuf, StoreError> {
    if let Ok(home) = std::env::var("HOME") {
        return Ok(PathBuf::from(home));
    }
    if let Ok(profile) = std::env::var("USERPROFILE") {
        return Ok(PathBuf::from(profile));
    }
    Err(StoreError::HomeDirUnresolved)
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

    /// Falsification target: `load_user_config` must surface a
    /// genuinely unreadable config path as `StoreError::Io` carrying
    /// the real path and the real `std::io::Error`, not a stringified
    /// `anyhow` message. A directory is stood in for the config file
    /// so the read fails at the OS level instead of short-circuiting
    /// on the `!path.exists()` default-config fast path.
    #[test]
    fn load_user_config_unreadable_path_returns_typed_io_error() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let home = MmcpHome::from_root(tmp.path());
        let config_path = home.user_config_path();
        std::fs::create_dir_all(&config_path)
            .expect("create dir standing in place of the config file");

        // Capture what the real syscall actually returns, so the
        // assertion below proves the source chain preserves the same
        // `ErrorKind` instead of collapsing it into a stand-in value.
        let expected_kind = std::fs::read_to_string(&config_path)
            .expect_err("reading a directory as a file must fail")
            .kind();

        let err = home
            .load_user_config()
            .expect_err("a directory standing in for the config file must not parse as one");

        match &err {
            StoreError::Io {
                path, operation, ..
            } => {
                assert_eq!(path, &config_path);
                assert_eq!(*operation, FileOperation::Read);
            }
            other => panic!("expected StoreError::Io, got {other:?}"),
        }
        let chained = std::error::Error::source(&err)
            .and_then(|s| s.downcast_ref::<std::io::Error>())
            .expect("source must be the real std::io::Error, not a stringified copy");
        assert_eq!(chained.kind(), expected_kind);
    }
}
