//! Failure modes around driving a native Tauri dialog.

use thiserror::Error;

/// Shared by every command that opens a native dialog.
#[derive(Debug, Error)]
pub enum GuiDialogError {
    /// No main window to parent a native dialog to.
    #[error("main window is not available")]
    NoMainWindow,

    /// The oneshot channel carrying a native dialog's result was
    /// dropped before the dialog callback fired.
    #[error("dialog channel closed before a result arrived")]
    ChannelClosed,

    /// The dialog returned a handle Tauri could not convert to a
    /// filesystem path (e.g. a non-`file://` URI).
    #[error("dialog returned an unusable path: {0}")]
    PathUnusable(String),
}
