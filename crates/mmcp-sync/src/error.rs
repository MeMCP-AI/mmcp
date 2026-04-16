//! Sync error type.

use thiserror::Error;
use uuid::Uuid;

/// Failures returned by the sync engine.
#[derive(Debug, Error)]
pub enum SyncError {
    /// The supplied semver string is not parseable.
    #[error("invalid semver: {0}")]
    InvalidVersion(#[from] semver::Error),

    /// The pending queue entry does not exist.
    #[error("pending edit not found: {0}")]
    NotFound(String),

    /// Git backend failure.
    #[error("git error: {0}")]
    Git(#[from] mmcp_git::GitError),

    /// HTTP transport failure while talking to the remote mmcp
    /// server (connection refused, TLS error, bad body, and so on).
    #[error("transport error: {0}")]
    Transport(String),

    /// The remote server answered with a non-success status and a
    /// human-readable reason, but the status was not a conflict.
    #[error("remote error ({status}): {message}")]
    Remote { status: u16, message: String },

    /// Push was rejected because the remote advanced under our
    /// feet. Callers should pull, re-apply the pending edit, and
    /// try again.
    ///
    /// The payload points at the memory that needs resolving plus
    /// the two commit ids so the CLI can print a minimal diff hint.
    #[error("push rejected: remote moved for memory {memory}")]
    Conflict {
        memory: Uuid,
        local_commit: String,
        remote_commit: String,
    },
}

impl SyncError {
    /// Construct a transport error with a formatted message.
    pub(crate) fn transport(e: impl std::fmt::Display) -> Self {
        Self::Transport(e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transport_constructor_formats_via_display() {
        let err = SyncError::transport("connection refused");
        match err {
            SyncError::Transport(msg) => assert_eq!(msg, "connection refused"),
            other => panic!("expected Transport, got {other:?}"),
        }
    }

    #[test]
    fn display_impls_emit_expected_prefixes() {
        assert_eq!(
            SyncError::NotFound("abc".into()).to_string(),
            "pending edit not found: abc"
        );
        assert_eq!(
            SyncError::Transport("boom".into()).to_string(),
            "transport error: boom"
        );
        assert_eq!(
            SyncError::Remote {
                status: 500,
                message: "oops".into()
            }
            .to_string(),
            "remote error (500): oops"
        );
        let mem = Uuid::nil();
        let conflict = SyncError::Conflict {
            memory: mem,
            local_commit: "abc".into(),
            remote_commit: "def".into(),
        }
        .to_string();
        assert!(conflict.contains("push rejected"));
        assert!(conflict.contains(&mem.to_string()));
    }

    #[test]
    fn semver_error_is_converted_via_from() {
        // A clearly invalid semver trips `semver::Version::parse`,
        // which `?`-propagates into `SyncError::InvalidVersion`
        // through the `#[from]` derive.
        let result: Result<semver::Version, SyncError> = "not-a-version"
            .parse::<semver::Version>()
            .map_err(Into::into);
        match result {
            Err(SyncError::InvalidVersion(_)) => {}
            other => panic!("expected InvalidVersion, got {other:?}"),
        }
    }

    #[test]
    fn git_error_is_converted_via_from() {
        let git_err = mmcp_git::GitError::RepoNotFound("/nowhere".into());
        let sync_err: SyncError = git_err.into();
        match sync_err {
            SyncError::Git(mmcp_git::GitError::RepoNotFound(p)) => assert_eq!(p, "/nowhere"),
            other => panic!("expected Git(RepoNotFound), got {other:?}"),
        }
    }
}
