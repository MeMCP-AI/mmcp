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

/// Credentials for outbound git transport operations.
///
/// The native backend applies these to the `git` subprocess it
/// spawns: [`Credentials::BearerHttp`] maps to an HTTP bearer header
/// via `-c http.extraHeader=...`, [`Credentials::SshCommand`] sets
/// `GIT_SSH_COMMAND`, and [`Credentials::None`] lets the user's
/// environment (SSH agent, credential helper, `.netrc`) decide —
/// the sensible default when mmcp is a plain git CLI wrapper.
///
/// Keep the enum non-exhaustive so backends that understand richer
/// credential shapes (mTLS, workload identity, forge-specific
/// tokens) can add variants later without breaking callers.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Credentials {
    /// No explicit credentials; rely on the ambient git environment
    /// (SSH agent, credential helper, `.netrc`, GCM, etc.). This is
    /// the default and the right choice for interactive use.
    #[default]
    None,

    /// HTTP bearer token. The backend sends `Authorization: Bearer <token>`
    /// with every request to the remote. Correct for GitHub/GitLab/Gitea
    /// personal access tokens and for mmcp-server's own push flow.
    BearerHttp(String),

    /// Exact value for the `GIT_SSH_COMMAND` env var — typically
    /// `ssh -i /path/to/key -o IdentitiesOnly=yes`. Lets callers point
    /// git at a specific key without touching the ambient SSH config.
    SshCommand(String),
}

impl Credentials {
    /// Shortcut for the most common case.
    #[must_use]
    pub fn none() -> Self {
        Credentials::None
    }

    /// Build HTTP bearer credentials from any string-like token.
    #[must_use]
    pub fn bearer(token: impl Into<String>) -> Self {
        Credentials::BearerHttp(token.into())
    }
}

/// Outcome of a local [`GitBackend::fast_forward`] call.
///
/// Fast-forward advances a local branch ref to match the commit at
/// another ref (typically a remote-tracking ref just populated by
/// `fetch`). The distinction between the variants matters to the
/// sync engine's pull path: `AlreadyAt` and `Advanced` both mean
/// "safe to publish as updated"; `NotFastForward` means local work
/// diverged from the remote and the caller needs to resolve the
/// split (step 7 of the sync plan upgrades this to the structured
/// `pull_diverged` error; today the engine records it silently).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FastForwardOutcome {
    /// `local_ref` already pointed at `target_ref`'s commit; the
    /// ref was left untouched.
    AlreadyAt {
        /// Shared commit id both refs agree on.
        commit: String,
    },
    /// `local_ref` was moved to the target commit. `from` is
    /// `None` when the local ref did not exist before the call
    /// (created-from-nothing case).
    Advanced {
        /// Prior commit the local ref was at, if any.
        from: Option<String>,
        /// New commit the local ref now points at.
        to: String,
    },
    /// `local_ref` is not an ancestor of `target_ref`. The caller
    /// must resolve the divergence explicitly; the ref was not
    /// modified.
    NotFastForward {
        /// Commit the local ref currently points at.
        local: String,
        /// Commit the target ref points at.
        target: String,
    },
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

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    #[test]
    fn repo_handle_new_stores_group_id_and_locator() {
        let uuid = Uuid::now_v7();
        let handle = RepoHandle::new(uuid, "/tmp/repo.git");
        assert_eq!(handle.group_id, uuid);
        assert_eq!(handle.locator, "/tmp/repo.git");
    }

    #[test]
    fn refspec_new_defaults_to_non_forced() {
        let r = RefSpec::new("refs/heads/main", "refs/heads/main");
        assert_eq!(r.local, "refs/heads/main");
        assert_eq!(r.remote, "refs/heads/main");
        assert!(!r.force);
    }

    #[test]
    fn refspec_forced_flips_the_flag() {
        let r = RefSpec::new("a", "b").forced();
        assert!(r.force);
    }

    #[test]
    fn rev_main_wraps_the_convention_constant() {
        assert_eq!(
            Rev::main(),
            Rev::Branch(mmcp_core::conventions::MAIN_BRANCH.to_string())
        );
    }

    #[test]
    fn rev_head_constructor_is_the_head_variant() {
        assert_eq!(Rev::head(), Rev::Head);
    }

    #[test]
    fn rev_canonical_covers_every_variant() {
        assert_eq!(Rev::Branch("main".into()).canonical(), "refs/heads/main");
        assert_eq!(Rev::Tag("v1.0.0".into()).canonical(), "refs/tags/v1.0.0");
        let hex = "deadbeef".repeat(5); // 40-char-ish hex
        assert_eq!(Rev::Commit(hex.clone()).canonical(), hex);
        assert_eq!(Rev::Head.canonical(), "HEAD");
    }

    #[test]
    fn rev_round_trips_through_serde_json() {
        for rev in [
            Rev::Branch("main".into()),
            Rev::Tag("v1".into()),
            Rev::Commit("abc".into()),
            Rev::Head,
        ] {
            let encoded = serde_json::to_string(&rev).expect("serialize Rev");
            let decoded: Rev = serde_json::from_str(&encoded).expect("deserialize Rev");
            assert_eq!(decoded, rev);
        }
    }

    #[test]
    fn commit_spec_mmcp_commit_targets_main_with_explicit_author() {
        let spec = CommitSpec::mmcp_commit(
            "test commit",
            vec![("memories/a.md".into(), Some(b"body".to_vec()))],
            "alice",
            "alice@example.com",
        );
        assert_eq!(spec.branch, mmcp_core::conventions::MAIN_BRANCH);
        assert_eq!(spec.author_name, "alice");
        assert_eq!(spec.author_email, "alice@example.com");
        assert_eq!(spec.message, "test commit");
        assert_eq!(spec.files.len(), 1);
        assert_eq!(spec.files[0].0, "memories/a.md");
    }

    #[test]
    fn credentials_default_is_none() {
        assert_eq!(Credentials::default(), Credentials::None);
        assert_eq!(Credentials::none(), Credentials::None);
    }

    #[test]
    fn credentials_bearer_wraps_the_token_verbatim() {
        let c = Credentials::bearer("ghp_test");
        assert_eq!(c, Credentials::BearerHttp("ghp_test".to_string()));
    }

    #[test]
    fn push_report_default_is_empty() {
        let report = PushReport::default();
        assert!(report.updated.is_empty());
        assert!(report.rejected.is_empty());
    }

    #[test]
    fn push_report_round_trips_through_serde_json() {
        let report = PushReport {
            updated: vec![("refs/heads/main".into(), "abc123".into())],
            rejected: vec![("refs/heads/stale".into(), "non-fast-forward".into())],
        };
        let encoded = serde_json::to_string(&report).expect("serialize");
        let decoded: PushReport = serde_json::from_str(&encoded).expect("deserialize");
        assert_eq!(decoded, report);
    }
}
