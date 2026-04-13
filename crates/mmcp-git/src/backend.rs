//! The pluggable git storage trait.

use async_trait::async_trait;
use bytes::Bytes;

use crate::error::GitError;
use crate::types::{CommitMeta, CommitSpec, GroupRef, PushReport, RefSpec, RepoHandle, Rev};

/// A pluggable git storage backend.
///
/// All methods are async because some backends (Forgejo, Gitea,
/// GitHub, GitLab) hit remote REST APIs, while others (the native
/// backend) talk to the local filesystem and wrap synchronous `gix`
/// calls in `spawn_blocking`.
#[async_trait]
pub trait GitBackend: Send + Sync {
    /// Create a new empty group repository.
    async fn create_group_repo(&self, group: &GroupRef) -> Result<RepoHandle, GitError>;

    /// Clone an existing repository into the provided directory.
    ///
    /// The native backend treats this as "make a non-bare working
    /// copy" and uses local filesystem operations. Remote clones are
    /// currently unsupported in the native backend and return
    /// [`GitError::Unsupported`].
    async fn clone_to(
        &self,
        repo: &RepoHandle,
        dst: &std::path::Path,
    ) -> Result<(), GitError>;

    /// Fetch updates for the named refs. A no-op on the native
    /// backend because there is no remote to fetch from.
    async fn fetch(
        &self,
        repo: &RepoHandle,
        refs: &[RefSpec],
    ) -> Result<(), GitError>;

    /// Push local refs to the backend. On the native backend this
    /// updates refs in the bare repo directly.
    async fn push(
        &self,
        repo: &RepoHandle,
        refs: &[RefSpec],
    ) -> Result<PushReport, GitError>;

    /// Read the raw bytes of a file inside the repository at a
    /// specific revision.
    async fn read_file(
        &self,
        repo: &RepoHandle,
        path: &str,
        rev: &Rev,
    ) -> Result<Bytes, GitError>;

    /// Write a new commit to the repository. The commit is applied on
    /// top of the current tip of `spec.branch`, or creates the branch
    /// if it does not yet exist. Returns the new commit id.
    async fn write_commit(
        &self,
        repo: &RepoHandle,
        spec: CommitSpec,
    ) -> Result<String, GitError>;

    /// Create a lightweight tag pointing at `target`.
    async fn tag(
        &self,
        repo: &RepoHandle,
        name: &str,
        target: &str,
    ) -> Result<(), GitError>;

    /// Walk the commit history that touches `path`, most recent first.
    async fn walk_history(
        &self,
        repo: &RepoHandle,
        path: &str,
    ) -> Result<Vec<CommitMeta>, GitError>;
}
