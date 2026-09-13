//! GUI error types.
//!
//! One file per error type.
//! See `crate::error::gui` for why the top-level [`GuiError`] facade exists.
//! [`GuiStoreError`] wraps the store-layer causes.
//! [`GuiDialogError`] wraps the native-dialog plumbing.
//! [`GuiArchiveError`] wraps the archive-command layer.
//! [`GuiError`] (plus [`GuiResult`]) is the single type every Tauri command surfaces on failure.
//! This module is re-exports only.

mod archive;
mod dialog;
mod gui;
mod store;

pub use archive::GuiArchiveError;
pub use dialog::GuiDialogError;
pub use gui::{GuiError, GuiResult};
pub use store::GuiStoreError;
