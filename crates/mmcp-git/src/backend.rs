//! The pluggable git storage trait.

use async_trait::async_trait;
use bytes::Bytes;
use mmcp_core::manifest::GroupManifest;

use crate::error::GitError;
use crate::types::{
    CommitMeta, CommitSpec, Credentials, FastForwardOutcome, PushReport, RefSpec, RepoHandle, Rev,
};

/// A pluggable git storage backend.
///
/// All methods are async because some backends (Forgejo, Gitea,
/// GitHub, GitLab) hit remote REST APIs, while others (the native
/// backend) talk to the local filesystem and wrap synchronous `gix`
/// calls in `spawn_blocking`.
#[async_trait]
pub trait GitBackend: Send + Sync {
    /// Create a new group repository and commit its initial
    /// `.mmcp.toml` manifest on the `main` branch.
    ///
    /// The method is idempotent with respect to repository
    /// initialization: calling it again with the same manifest on
    /// an existing repo returns the same handle without writing a
    /// new commit.
    async fn create_group_repo(&self, manifest: &GroupManifest) -> Result<RepoHandle, GitError>;

    /// Read and parse the `.mmcp.toml` manifest from the repo's
    /// `main` branch.
    async fn read_manifest(&self, repo: &RepoHandle) -> Result<GroupManifest, GitError>;

    /// Commit a new revision of the `.mmcp.toml` manifest on the
    /// repo's `main` branch. Used by disaster recovery and by
    /// admin flows that need to rename or reclassify a group.
    async fn write_manifest(
        &self,
        repo: &RepoHandle,
        manifest: &GroupManifest,
    ) -> Result<String, GitError>;

    /// Clone a remote repository into the provided directory.
    ///
    /// `remote_url` is any git-compatible endpoint (mmcp-server,
    /// GitHub, Gitea, self-hosted, `file://`, …). The native
    /// backend shells out to the user-installed `git` binary so
    /// the operation works against any smart-HTTP-capable server
    /// without bringing in an HTTP client dependency.
    ///
    /// `creds` selects the authentication mechanism: see
    /// [`Credentials`] for the variants. Use [`Credentials::None`]
    /// to rely on the ambient git environment.
    async fn clone_to(
        &self,
        remote_url: &str,
        dst: &std::path::Path,
        creds: &Credentials,
    ) -> Result<(), GitError>;

    /// Fetch the named refs from `remote_url` into `repo`. The
    /// native backend registers (or updates) an `origin` remote
    /// pointing at `remote_url` and invokes `git fetch` on it.
    async fn fetch(
        &self,
        repo: &RepoHandle,
        remote_url: &str,
        refs: &[RefSpec],
        creds: &Credentials,
    ) -> Result<(), GitError>;

    /// Push the named refs from `repo` to `remote_url`. Same
    /// mechanism as `fetch`: the native backend shells out to
    /// `git push origin <refs>` after ensuring the `origin`
    /// remote points at `remote_url`.
    async fn push(
        &self,
        repo: &RepoHandle,
        remote_url: &str,
        refs: &[RefSpec],
        creds: &Credentials,
    ) -> Result<PushReport, GitError>;

    /// Read the raw bytes of a file inside the repository at a
    /// specific revision.
    async fn read_file(&self, repo: &RepoHandle, path: &str, rev: &Rev) -> Result<Bytes, GitError>;

    /// Write a new commit to the repository. The commit is applied on
    /// top of the current tip of `spec.branch`, or creates the branch
    /// if it does not yet exist. Returns the new commit id.
    async fn write_commit(&self, repo: &RepoHandle, spec: CommitSpec) -> Result<String, GitError>;

    /// Create a lightweight tag pointing at `target`.
    async fn tag(&self, repo: &RepoHandle, name: &str, target: &str) -> Result<(), GitError>;

    /// Fast-forward `local_ref` to the commit pointed at by
    /// `target_ref`.
    ///
    /// Pure-local ref manipulation: both refs must already exist
    /// in the repo (the caller is expected to have run `fetch`
    /// first to populate `target_ref`, typically the
    /// `refs/remotes/origin/<branch>` tracking ref).
    ///
    /// Returns a [`FastForwardOutcome`] distinguishing the three
    /// relevant cases: `AlreadyAt` when the refs already agree,
    /// `Advanced` when the local ref was moved (or created from
    /// nothing), and `NotFastForward` when local is not an
    /// ancestor of target and the caller has to resolve the
    /// divergence. The ref is not modified in the `NotFastForward`
    /// case.
    async fn fast_forward(
        &self,
        repo: &RepoHandle,
        local_ref: &str,
        target_ref: &str,
    ) -> Result<FastForwardOutcome, GitError>;

    /// Walk the commit history that touches `path`, most recent first.
    async fn walk_history(
        &self,
        repo: &RepoHandle,
        path: &str,
    ) -> Result<Vec<CommitMeta>, GitError>;

    /// List every blob directly under `path_prefix` at the given
    /// revision.
    ///
    /// Returned values are the file names *relative to* `path_prefix`
    /// (i.e. without the prefix itself). Subtrees under the prefix
    /// are not recursed into. Use an empty `path_prefix` for the
    /// tree root.
    ///
    /// An empty tree or a prefix that does not exist at the given
    /// revision returns an empty vector, not an error, so callers
    /// can treat "no memories yet" as the normal case.
    async fn list_tree(
        &self,
        repo: &RepoHandle,
        path_prefix: &str,
        rev: &Rev,
    ) -> Result<Vec<String>, GitError>;

    /// List every subtree (directory) directly under `path_prefix`
    /// at the given revision. Mirror of [`list_tree`] but for
    /// directory entries, so resolvers can enumerate slug
    /// directories under `memories/`.
    ///
    /// Returned values are the subtree names *relative to*
    /// `path_prefix`. Nested subtrees are not recursed into.
    /// Missing prefix returns an empty vector.
    async fn list_subtrees(
        &self,
        repo: &RepoHandle,
        path_prefix: &str,
        rev: &Rev,
    ) -> Result<Vec<String>, GitError>;
}
