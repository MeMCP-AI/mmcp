//! [`PushReport`], its per-remote [`RemotePushOutcome`], its
//! [`PushTransportError`] cause, and its per-group item
//! [`PushedGroup`].

use thiserror::Error;
use uuid::Uuid;

use super::GroupSyncFailure;

/// Report of a completed `push` call.
///
/// One [`RemotePushOutcome`] per [`crate::PushScope`]-selected
/// remote the push actually targeted, in the engine's own remote
/// list order. Every caller that needs a single combined view uses
/// [`PushReport::total_pushed`] / [`PushReport::total_failed`] /
/// [`PushReport::iter_failures`] rather than re-flattening
/// `by_remote` by hand at each call site.
#[derive(Debug)]
pub struct PushReport {
    /// One outcome per targeted remote.
    pub by_remote: Vec<RemotePushOutcome>,
}

impl PushReport {
    /// Total groups pushed (successfully attempted, regardless of
    /// `content_transferred`) across every remote.
    #[must_use]
    pub fn total_pushed(&self) -> usize {
        self.by_remote.iter().map(|r| r.pushed.len()).sum()
    }

    /// Total groups whose push attempt itself errored across every
    /// remote.
    #[must_use]
    pub fn total_failed(&self) -> usize {
        self.by_remote.iter().map(|r| r.failed.len()).sum()
    }

    /// Every `(remote_name, failure)` pair across every remote, in
    /// `by_remote` order.
    pub fn iter_failures(&self) -> impl Iterator<Item = (&str, &GroupSyncFailure)> {
        self.by_remote
            .iter()
            .flat_map(|r| r.failed.iter().map(move |f| (r.remote_name.as_str(), f)))
    }

    /// Every `(remote_name, pushed_group)` pair whose content-plane
    /// transfer was skipped (`content_transferred: false`), across
    /// every remote, in `by_remote` order.
    pub fn iter_partial_failures(&self) -> impl Iterator<Item = (&str, &PushedGroup)> {
        self.by_remote.iter().flat_map(|r| {
            r.pushed
                .iter()
                .filter(|p| !p.content_transferred)
                .map(move |p| (r.remote_name.as_str(), p))
        })
    }
}

/// One remote's push outcome: which groups it shipped, which it
/// failed on. `SyncError` is not `Clone`/`PartialEq` (it wraps
/// `mmcp_git::GitError`, which wraps `std::io::Error`), so this
/// carries no derive beyond `Debug`; nothing in this workspace
/// compares or clones a full report (field-level assertions cover
/// the tests).
#[derive(Debug)]
pub struct RemotePushOutcome {
    /// Name of the [`crate::BoundRemote`] this outcome belongs to.
    pub remote_name: String,
    /// Groups whose push attempt did NOT error, in iteration order.
    /// A group appears here even when the content plane was
    /// skipped (see `PushedGroup::content_transferred`); a group
    /// whose push attempt itself errored appears in `failed`
    /// instead, never here.
    pub pushed: Vec<PushedGroup>,
    /// Groups whose push attempt itself errored (not merely a
    /// content-plane transport skip, which still counts as a
    /// successful `PushedGroup` with `content_transferred: false`).
    /// Every OTHER scheduled group still ran to completion regardless
    /// of a group appearing here; see [`GroupSyncFailure`].
    pub failed: Vec<GroupSyncFailure>,
}

/// Why a [`PushedGroup`]'s content-plane transfer was skipped.
/// Populated only when [`PushedGroup::content_transferred`] is
/// `false`; mirrors the two backend conditions that can cause that
/// (an unsupported operation, or a failed transport subprocess) as
/// distinct variants rather than one flattened message.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum PushTransportError {
    /// The backend declined the content-plane push outright.
    #[error("operation not supported by this backend: {reason}")]
    Unsupported { reason: &'static str },
    /// The `git` subprocess driving the push exited non-zero.
    #[error("git {op} against {url} failed: {stderr}")]
    Transport {
        op: &'static str,
        url: String,
        stderr: String,
    },
}

/// One pushed group's before/after snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PushedGroup {
    pub group_id: Uuid,
    /// `false` when the content-plane push was skipped (backend
    /// returned `Unsupported` or a transport error). The group
    /// still appears in the report so operators can see what was
    /// attempted; retry on the next push picks it up.
    pub content_transferred: bool,
    /// The reason the content-plane transfer was skipped. `None`
    /// when `content_transferred` is `true`.
    pub transport_error: Option<PushTransportError>,
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use crate::error::SyncError;
    use mmcp_core::id::GroupId;

    fn sample_report() -> PushReport {
        PushReport {
            by_remote: vec![
                RemotePushOutcome {
                    remote_name: "primary".to_string(),
                    pushed: vec![
                        PushedGroup {
                            group_id: Uuid::nil(),
                            content_transferred: true,
                            transport_error: None,
                        },
                        PushedGroup {
                            group_id: Uuid::max(),
                            content_transferred: false,
                            transport_error: Some(PushTransportError::Transport {
                                op: "push",
                                url: "https://example.test/g".to_string(),
                                stderr: "connection reset".to_string(),
                            }),
                        },
                    ],
                    failed: vec![GroupSyncFailure {
                        group_id: GroupId::from_uuid(Uuid::now_v7()),
                        error: SyncError::NotFound("x".to_string()),
                    }],
                },
                RemotePushOutcome {
                    remote_name: "mirror".to_string(),
                    pushed: vec![],
                    failed: vec![],
                },
            ],
        }
    }

    #[test]
    fn total_pushed_sums_across_remotes() {
        assert_eq!(sample_report().total_pushed(), 2);
    }

    #[test]
    fn total_failed_sums_across_remotes() {
        assert_eq!(sample_report().total_failed(), 1);
    }

    #[test]
    fn iter_failures_attributes_each_failure_to_its_remote() {
        let report = sample_report();
        let names: Vec<&str> = report.iter_failures().map(|(name, _)| name).collect();
        assert_eq!(names, vec!["primary"]);
    }

    #[test]
    fn iter_partial_failures_only_yields_untransferred_groups() {
        let report = sample_report();
        let partials: Vec<(&str, Uuid)> = report
            .iter_partial_failures()
            .map(|(name, g)| (name, g.group_id))
            .collect();
        assert_eq!(partials, vec![("primary", Uuid::max())]);
    }
}
