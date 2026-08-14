//! Tempdir-backed fixtures shared across every `mmcp-store` consumer.
//!
//! The goal is one canonical way to boot the store into a throwaway state, used by:
//!
//! - This crate's own `#[cfg(test)]` unit tests.
//! - `mmcp-client`'s integration suite under `crates/mmcp-client/tests/`.
//! - `mmcp-gui`'s widget tests: this surface stays stable so that crate can pick it up unchanged.
//! - Any future third-party consumer driving the store programmatically.
//!
//! Gated behind the `testing` Cargo feature,
//! so downstream crates opt in via `mmcp-store = { ..., features = ["testing"] }`,
//! and release builds don't pay the `tempfile` compile cost.
//! A `#[cfg(test)]` equivalent inside the source tree wouldn't let outside crates reach these helpers,
//! hence the feature flag.
//!
//! The helpers intentionally hold onto their `TempDir` handles:
//! when the fixture goes out of scope the backing directory is removed.
//! That keeps tests leak-free without any explicit teardown.

use std::path::PathBuf;
use std::sync::Arc;

use mmcp_core::id::GroupId;
use mmcp_core::manifest::GroupManifest;
use mmcp_git::{GitBackend, NativeBackend};
use tempfile::TempDir;
use uuid::Uuid;

use crate::error::StoreError;
use crate::groups::GroupIndex;
use crate::home::{MmcpHome, ResolvedAuthor};
use crate::sessions::SessionStore;

/// An [`MmcpHome`] rooted inside a fresh tempdir, pre-wired with a backend, group index, and session store.
///
/// The `TempDir` is kept alive by the fixture so dropping the `ScratchHome` cleans the directory.
/// Fields are exposed as references so callers can drive the usual store surface without cloning.
pub struct ScratchHome {
    home: MmcpHome,
    backend: Arc<NativeBackend>,
    groups: GroupIndex,
    sessions: SessionStore,
    author: ResolvedAuthor,
    _tmp: TempDir,
}

impl ScratchHome {
    /// Build a scratch home and fully initialise its backend, group index, and session store.
    ///
    /// Errors surface as [`StoreError`] so callers can match on the canonical store-side error shape,
    /// without juggling intermediate conversions.
    /// Propagated backend/index failures only happen in genuinely pathological test setups (e.g. a poisoned tempdir),
    /// so most call sites `.expect("scratch home")` and move on.
    pub async fn new() -> Result<Self, StoreError> {
        let tmp = TempDir::new()?;
        let home = MmcpHome::from_root(tmp.path().join("mmcp-home"));
        std::fs::create_dir_all(home.repos_root())?;
        let (backend, groups) = home.init_backend().await.map_err(std::io::Error::other)?;
        let sessions = SessionStore::open(home.sessions_root())?;
        let author = ephemeral_author();
        Ok(Self {
            home,
            backend,
            groups,
            sessions,
            author,
            _tmp: tmp,
        })
    }

    /// The underlying [`MmcpHome`], for path derivations or author refresh.
    #[must_use]
    pub fn home(&self) -> &MmcpHome {
        &self.home
    }

    /// Shared [`NativeBackend`] handle.
    /// Clone via `Arc` if the test needs to hand it to a task that outlives the fixture borrow lifetime.
    #[must_use]
    pub fn backend(&self) -> &Arc<NativeBackend> {
        &self.backend
    }

    /// The live [`GroupIndex`].
    /// Refreshed lazily by the fixture methods that create groups;
    /// callers can also poke `.refresh()` on it directly.
    #[must_use]
    pub fn groups(&self) -> &GroupIndex {
        &self.groups
    }

    /// The flat-TOML [`SessionStore`] rooted under this scratch home.
    #[must_use]
    pub fn sessions(&self) -> &SessionStore {
        &self.sessions
    }

    /// An [`ephemeral_author`] snapshot captured at construction time.
    /// Returned by reference to match the real CRUD surface.
    #[must_use]
    pub fn author(&self) -> &ResolvedAuthor {
        &self.author
    }

    /// The repos root (`<tmp>/mmcp-home/repos`).
    /// Tests that need to poke at raw bare repos on disk should use this rather than recomputing the path.
    #[must_use]
    pub fn repos_root(&self) -> PathBuf {
        self.home.repos_root()
    }

    /// Create a fresh bare repo backed by a new [`GroupManifest`],
    /// and refresh the group index so callers can resolve the new entry immediately.
    pub async fn seed_group(&self, slug: &str) -> Result<SeededGroup, StoreError> {
        let group_id = GroupId::new();
        let owner = Uuid::now_v7();
        let manifest = GroupManifest::new_user_owned(group_id, slug, owner);
        self.backend.create_group_repo(&manifest).await?;
        self.groups.refresh().await?;
        Ok(SeededGroup {
            group_id,
            owner,
            manifest,
        })
    }
}

/// Return value of [`ScratchHome::seed_group`]:
/// enough to drive memory CRUD against the freshly-created group without re-walking the index.
#[derive(Debug, Clone)]
pub struct SeededGroup {
    /// Stable [`GroupId`] of the new group.
    pub group_id: GroupId,
    /// Randomly-minted owner UUID stored inside the manifest.
    /// Exposed because tests assert on it when they want to distinguish groups.
    pub owner: Uuid,
    /// The full manifest as written on disk.
    pub manifest: GroupManifest,
}

/// Build a [`ResolvedAuthor`] with stable, non-real credentials.
///
/// Kept ephemeral, same name/email every time,
/// so tests that assert on commit metadata can compare against a known pair,
/// without touching the operator's real git config.
#[must_use]
pub fn ephemeral_author() -> ResolvedAuthor {
    ResolvedAuthor {
        name: "mmcp-test".to_string(),
        email: "mmcp-test@example.invalid".to_string(),
    }
}
