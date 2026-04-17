//! UI-owned view of the sync subsystem's state.
//!
//! The worker only emits the outcomes of individual pull / push
//! calls; the UI aggregates them into a user-visible state machine
//! it drives itself. That keeps the worker's outcome surface small
//! and makes it trivial for the status bar to show the "currently
//! syncing…" state without needing a separate worker ping.

#[derive(Debug, Clone, Copy)]
pub enum SyncOp {
    Pull,
    Push,
}

impl SyncOp {
    pub const fn as_str(self) -> &'static str {
        match self {
            SyncOp::Pull => "pull",
            SyncOp::Push => "push",
        }
    }
}

#[derive(Debug, Clone, Default)]
pub enum SyncStatus {
    /// Initial state before the worker has reported its config.
    #[default]
    Unknown,
    /// No `.mmcp.toml` with a `[sync]` block on any cwd ancestor.
    NotConfigured,
    /// Configured and not currently syncing.
    Idle { server_url: String },
    /// Pull or push is in flight.
    Syncing { server_url: String, op: SyncOp },
    /// Last operation succeeded.
    LastOk {
        server_url: String,
        op: SyncOp,
        summary: String,
    },
    /// Last operation failed.
    LastErr {
        server_url: String,
        op: SyncOp,
        message: String,
    },
}

impl SyncStatus {
    /// True when Pull / Push buttons should be enabled.
    pub fn is_ready(&self) -> bool {
        matches!(
            self,
            SyncStatus::Idle { .. } | SyncStatus::LastOk { .. } | SyncStatus::LastErr { .. }
        )
    }

    pub fn server_url(&self) -> Option<&str> {
        match self {
            SyncStatus::Idle { server_url }
            | SyncStatus::Syncing { server_url, .. }
            | SyncStatus::LastOk { server_url, .. }
            | SyncStatus::LastErr { server_url, .. } => Some(server_url),
            SyncStatus::Unknown | SyncStatus::NotConfigured => None,
        }
    }
}
