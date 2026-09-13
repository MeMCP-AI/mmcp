//! Tempdir-backed fixtures shared across every `mmcp-store` consumer.
//!
//! The goal is one canonical way to boot the store into a throwaway state, used by:
//!
//! - This crate's own `#[cfg(test)]` unit tests.
//! - `mmcp-client`'s integration suite under `crates/mmcp-client/tests/`.
//! - `mmcp-gui`'s widget tests: this surface stays stable so that crate can pick it up unchanged.
//! - Any future third-party consumer driving the store programmatically.
//!
//! Gated behind the `testing` Cargo feature.
//! Downstream crates opt in via `mmcp-store = { ..., features = ["testing"] }`.
//! That keeps a release build from paying the `tempfile` compile cost.
//! A `#[cfg(test)]` equivalent inside the source tree wouldn't let outside crates reach these helpers.
//! Hence the feature flag.
//!
//! The helpers intentionally hold onto their `TempDir` handles.
//! When the fixture goes out of scope, the backing directory is removed.
//! That keeps tests leak-free without any explicit teardown.

use std::path::PathBuf;
use std::sync::Arc;

use mmcp_core::id::{GroupId, UserId};
use mmcp_core::manifest::GroupManifest;
use mmcp_core::memory::{BumpIntent, MemoryFrontmatter, MemoryRef};
use mmcp_git::{CommitSpec, GitBackend, NativeBackend, RepoHandle, Rev};
use tempfile::TempDir;
use uuid::Uuid;

use crate::error::{FileOperation, StoreError};
use crate::groups::GroupIndex;
use crate::home::{MmcpHome, ResolvedAuthor};
use crate::memory::{ImportError, read_frontmatter_at, resolve_memory};
use crate::sessions::SessionStore;

mod corrupt_seed_error;

pub use corrupt_seed_error::CorruptSeedError;

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
    /// Errors surface as [`StoreError`] so callers can match on the canonical store-side error shape.
    /// That avoids juggling intermediate conversions.
    /// Propagated backend/index failures only happen in genuinely pathological test setups, e.g. a poisoned tempdir.
    /// Most call sites `.expect("scratch home")` and move on.
    pub async fn new() -> Result<Self, StoreError> {
        // `TempDir::new` retries several randomly named candidates internally.
        // It does not expose which one failed.
        // So this names the OS temp root instead of misreporting the exact path that failed.
        // See `StoreError::TempDirUnavailable`'s doc comment.
        let tmp = TempDir::new().map_err(|source| StoreError::TempDirUnavailable {
            root: std::env::temp_dir(),
            source,
        })?;
        let home = MmcpHome::from_root(tmp.path().join("mmcp-home"));
        std::fs::create_dir_all(home.repos_root()).map_err(|source| {
            StoreError::io(home.repos_root(), FileOperation::CreateDir, source)
        })?;
        let (backend, groups) = home.init_backend().await?;
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
    /// Refreshed lazily by the fixture methods that create groups; callers can also poke `.refresh()` on it directly.
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

    /// Create a fresh bare repo backed by a new [`GroupManifest`].
    /// Refresh the group index so callers can resolve the new entry immediately.
    pub async fn seed_group(&self, slug: &str) -> Result<SeededGroup, StoreError> {
        let group_id = GroupId::new();
        let owner = Uuid::now_v7();
        let manifest = GroupManifest::new_user_owned(group_id, slug, UserId::from_uuid(owner));
        self.backend.create_group_repo(&manifest).await?;
        self.groups.refresh().await?;
        Ok(SeededGroup {
            group_id,
            owner,
            manifest,
        })
    }
}

/// Return value of [`ScratchHome::seed_group`].
/// Enough to drive memory CRUD against the freshly-created group without re-walking the index.
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
/// Kept ephemeral, same name/email every time.
/// Tests that assert on commit metadata can compare against a known pair.
/// This never touches the operator's real git config.
#[must_use]
pub fn ephemeral_author() -> ResolvedAuthor {
    ResolvedAuthor {
        name: "mmcp-test".to_string(),
        email: "mmcp-test@example.invalid".to_string(),
    }
}

/// Resolve `slug` to its on-disk memory and return its raw frontmatter.
///
/// A test assertion often needs a field a tracker's own record shape omits.
/// Examples: `tags`, `refs`, `source`, `bump_intent`, `version`.
/// This pairs [`resolve_memory`] with the crate's frontmatter reader in one call.
pub async fn read_current_frontmatter(
    backend: &NativeBackend,
    handle: &RepoHandle,
    slug: &str,
) -> Result<MemoryFrontmatter, ImportError> {
    let resolved = resolve_memory(backend, handle, Some(slug), None).await?;
    read_frontmatter_at(backend, handle, &Rev::head(), &resolved.path).await
}

/// Set every tracker-non-owned [`MemoryFrontmatter`] field to a non-default value.
///
/// A field left at its type default (`Vec::new()`, `false`, `None`) cannot prove preservation.
/// A bug that resets the field to its default would pass unnoticed.
/// `id`, `name`, `description`, `kind`, and the tracker metadata blocks are not modified here.
/// Callers set those through the ordinary [`MemoryFrontmatter`] constructors.
#[must_use]
pub fn seed_unowned_fields(frontmatter: MemoryFrontmatter) -> MemoryFrontmatter {
    // "1.2.3" is a fixed valid semver literal: this parse can never fail.
    #[allow(clippy::expect_used)]
    let version = "1.2.3".parse().expect("valid semver literal");
    frontmatter
        .with_tags(vec!["alpha".to_string(), "beta".to_string()])
        .with_mandatory(true)
        .with_bump_intent(Some(BumpIntent::Patch))
        .with_source(Some(Uuid::now_v7()))
        .with_version(Some(version))
        .with_refs(vec![MemoryRef::new(Uuid::now_v7(), "deadbeefcafe")])
}

/// Read `path`'s raw bytes at `HEAD`.
pub async fn read_raw_bytes(
    backend: &NativeBackend,
    handle: &RepoHandle,
    path: &str,
) -> Result<Vec<u8>, mmcp_git::GitError> {
    Ok(backend
        .read_file(handle, path, &Rev::head())
        .await?
        .to_vec())
}

/// Overwrite `path` with `bytes` in a new commit, as an operator's raw git surgery would.
pub async fn overwrite_raw_bytes(
    backend: &NativeBackend,
    handle: &RepoHandle,
    path: &str,
    bytes: Vec<u8>,
    author: &ResolvedAuthor,
) -> Result<(), mmcp_git::GitError> {
    backend
        .write_commit(
            handle,
            CommitSpec::mmcp_commit(
                format!("test: overwrite {path}"),
                vec![(path.to_string(), Some(bytes))],
                &author.name,
                &author.email,
            ),
        )
        .await?;
    Ok(())
}

/// UTF-8 continuation byte (`10xxxxxx`): invalid wherever a character starts.
///
/// `0x80` never starts a valid UTF-8 sequence, so writing it at a char boundary breaks decoding there.
const UTF8_CONTINUATION_BYTE: u8 = 0x80;

/// Corrupt `source` into invalid UTF-8 at the byte offset where `marker` starts.
///
/// Replaces that one byte with `UTF8_CONTINUATION_BYTE`.
/// `marker` pins the corruption to a caller-chosen region, e.g. a frontmatter field or the body.
/// A test can target the frontmatter block or the body at will by choosing where `marker` sits.
/// Returns `None` when `marker` is not found in `source`.
#[must_use]
pub fn corrupt_one_byte(source: &str, marker: &str) -> Option<Vec<u8>> {
    let offset = source.find(marker)?;
    let mut bytes = source.as_bytes().to_vec();
    bytes[offset] = UTF8_CONTINUATION_BYTE;
    Some(bytes)
}

/// Corrupt `path`'s stored file at `marker`, in a new commit, and return the corrupted bytes.
///
/// Reads the current bytes and corrupts one via [`corrupt_one_byte`].
/// The result is written back via [`overwrite_raw_bytes`].
/// Returns the corrupted bytes so a caller can assert a rejected write left the stored file unchanged.
pub async fn corrupt_stored_file(
    backend: &NativeBackend,
    handle: &RepoHandle,
    path: &str,
    marker: &str,
    author: &ResolvedAuthor,
) -> Result<Vec<u8>, CorruptSeedError> {
    let valid_bytes = read_raw_bytes(backend, handle, path).await?;
    let valid_text = String::from_utf8(valid_bytes).map_err(CorruptSeedError::NotUtf8)?;
    let corrupted =
        corrupt_one_byte(&valid_text, marker).ok_or(CorruptSeedError::MarkerNotFound)?;
    overwrite_raw_bytes(backend, handle, path, corrupted.clone(), author).await?;
    Ok(corrupted)
}

/// Assert that `after` differs from `before` in exactly the fields `apply_owned` mutates.
///
/// Clones `before`, applies `apply_owned` to the clone, and compares against `after`.
/// Comparison is field by field via [`MemoryFrontmatter`]'s `PartialEq`.
/// Covers every field the struct declares, including a field added later.
pub fn assert_update_changed_only(
    before: &MemoryFrontmatter,
    after: &MemoryFrontmatter,
    apply_owned: impl FnOnce(&mut MemoryFrontmatter),
) {
    let mut expected = before.clone();
    apply_owned(&mut expected);
    assert_eq!(
        after, &expected,
        "an update must change only the fields it owns"
    );
}
