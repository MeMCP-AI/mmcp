//! Value types shared by every [`GitBackend`](crate::GitBackend)
//! implementation.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Opaque handle to a repository returned by a backend.
///
/// Holds whatever addressing information the backend needs to operate
/// on the repo: for the native backend that is the filesystem path;
/// for forge-backed backends that is the REST repo slug.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoHandle {
    /// Stable identifier of the underlying group.
    pub group_id: Uuid,

    /// Backend-specific locator (path, URL, etc.).
    pub locator: String,
}

impl RepoHandle {
    #[must_use]
    pub fn new(group_id: Uuid, locator: impl Into<String>) -> Self {
        Self {
            group_id,
            locator: locator.into(),
        }
    }
}

/// A git ref to fetch or push.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RefSpec {
    /// Source ref as the local side sees it (e.g. `refs/heads/main`).
    pub local: String,

    /// Remote ref the operation should target.
    pub remote: String,

    /// If true, allow non-fast-forward updates on push.
    pub force: bool,
}

impl RefSpec {
    #[must_use]
    pub fn new(local: impl Into<String>, remote: impl Into<String>) -> Self {
        Self {
            local: local.into(),
            remote: remote.into(),
            force: false,
        }
    }

    #[must_use]
    pub fn forced(mut self) -> Self {
        self.force = true;
        self
    }
}

/// A revision inside a repository.
///
/// A `Rev` can be a branch name, a tag name, a raw commit id, or the
/// repo's current `HEAD` (whatever branch it points to). The backend
/// is responsible for interpreting it.
///
/// Use `Rev::Head` for reads against a repository whose default branch
/// name is not known in advance (foreign clones, repos cloned from a
/// `master`-based remote, etc.). Use [`Rev::main`] only when mmcp owns
/// the repo's policy and knows it was created with `main`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Rev {
    /// Branch by name, without the `refs/heads/` prefix.
    Branch(String),

    /// Tag by name, without the `refs/tags/` prefix.
    Tag(String),

    /// Commit by hex id.
    Commit(String),

    /// Whatever branch `HEAD` currently points at.
    Head,
}

impl Rev {
    /// The default branch revision mmcp uses when creating repos
    /// itself (`main`). Prefer [`Rev::head`] on read paths so foreign
    /// repos with non-`main` defaults still resolve correctly.
    #[must_use]
    pub fn main() -> Self {
        Rev::Branch(mmcp_core::conventions::MAIN_BRANCH.to_string())
    }

    /// Whatever the repo's `HEAD` currently points at. Use this for
    /// reads when the caller does not know (or care) which branch name
    /// the repo actually uses — safe against any default-branch
    /// convention.
    #[must_use]
    pub fn head() -> Self {
        Rev::Head
    }

    /// Canonical form used for diagnostics.
    #[must_use]
    pub fn canonical(&self) -> String {
        match self {
            Rev::Branch(name) => format!("refs/heads/{name}"),
            Rev::Tag(name) => format!("refs/tags/{name}"),
            Rev::Commit(id) => id.clone(),
            Rev::Head => "HEAD".to_string(),
        }
    }
}

/// Specification for a new commit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommitSpec {
    /// Branch to advance. If the branch does not exist yet, it is
    /// created pointing at the new commit.
    pub branch: String,

    /// Author display name.
    pub author_name: String,

    /// Author email address.
    pub author_email: String,

    /// Commit message.
    pub message: String,

    /// File edits to apply to the index before committing. The key is
    /// the path relative to the repo root; `None` means delete.
    pub files: Vec<(String, Option<Vec<u8>>)>,
}

impl CommitSpec {
    /// Build a commit on the main branch with explicit author.
    ///
    /// `author_name` and `author_email` come from the resolved
    /// author cascade (user config -> git config -> fallback).
    #[must_use]
    pub fn mmcp_commit(
        message: impl Into<String>,
        files: Vec<(String, Option<Vec<u8>>)>,
        author_name: &str,
        author_email: &str,
    ) -> Self {
        Self {
            branch: mmcp_core::conventions::MAIN_BRANCH.to_string(),
            author_name: author_name.to_string(),
            author_email: author_email.to_string(),
            message: message.into(),
            files,
        }
    }
}

/// Report returned from a push operation.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct PushReport {
    /// Refs that were successfully updated, as `(ref, new_commit)`.
    pub updated: Vec<(String, String)>,

    /// Refs that the server rejected, as `(ref, reason)`.
    pub rejected: Vec<(String, String)>,
}

/// Metadata about a historical commit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommitMeta {
    /// Commit hex identifier.
    pub id: String,

    /// First line of the commit message.
    pub subject: String,

    /// Full commit message.
    pub message: String,

    /// Author display name.
    pub author_name: String,

    /// Author email.
    pub author_email: String,

    /// Seconds since the Unix epoch.
    pub timestamp: i64,
}
