//! [`SyncReport`].

use super::{PullReport, PushReport};

/// Report of a full sync (pull then push).
#[derive(Debug)]
pub struct SyncReport {
    pub pulled: PullReport,
    pub pushed: PushReport,
}
