//! Unified GUI error type.
//!
//! Every command surfaces this type on failure. Tauri auto-converts
//! via the `Serialize` impl into a JSON payload the frontend can
//! discriminate on `kind`. Keep variants coarse — the frontend
//! treats most failures the same (toast the message); the variants
//! matter when UI logic needs to branch (e.g. "not configured" vs
//! "connection refused").

use serde::Serialize;
use thiserror::Error;

#[derive(Debug, Error, Serialize)]
#[serde(tag = "kind", content = "message", rename_all = "snake_case")]
pub enum GuiError {
    #[error("store: {0}")]
    Store(String),

    #[error("git: {0}")]
    Git(String),

    #[error("sync: {0}")]
    Sync(String),

    #[error("sync-not-configured")]
    SyncNotConfigured,

    #[error("{0}")]
    Other(String),
}

impl From<mmcp_store::StoreError> for GuiError {
    fn from(e: mmcp_store::StoreError) -> Self {
        GuiError::Store(e.to_string())
    }
}

impl From<mmcp_git::GitError> for GuiError {
    fn from(e: mmcp_git::GitError) -> Self {
        GuiError::Git(e.to_string())
    }
}

impl From<mmcp_store::ImportError> for GuiError {
    fn from(e: mmcp_store::ImportError) -> Self {
        GuiError::Store(e.to_string())
    }
}

impl From<mmcp_store::ArchiveError> for GuiError {
    fn from(e: mmcp_store::ArchiveError) -> Self {
        GuiError::Store(e.to_string())
    }
}

impl From<anyhow::Error> for GuiError {
    fn from(e: anyhow::Error) -> Self {
        GuiError::Other(e.to_string())
    }
}

impl From<mmcp_core::memory::MemoryParseError> for GuiError {
    fn from(e: mmcp_core::memory::MemoryParseError) -> Self {
        GuiError::Store(e.to_string())
    }
}

impl From<std::str::Utf8Error> for GuiError {
    fn from(e: std::str::Utf8Error) -> Self {
        GuiError::Other(format!("utf-8: {e}"))
    }
}

pub type GuiResult<T> = std::result::Result<T, GuiError>;
