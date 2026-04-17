//! GUI-level error type.
//!
//! `GuiError` is the single failure shape the UI layer reasons about.
//! It wraps every distinct error a background task can produce (git,
//! store, memory CRUD, frontmatter parse, UTF-8 decode) into one
//! enum so toasts and status indicators have a uniform `Display`
//! target.

use mmcp_core::memory::MemoryParseError;
use mmcp_git::GitError;
use mmcp_store::{ImportError, StoreError};

#[derive(Debug, thiserror::Error)]
pub enum GuiError {
    #[error("store: {0}")]
    Store(#[from] StoreError),

    #[error("git: {0}")]
    Git(#[from] GitError),

    #[error("memory: {0}")]
    Memory(#[from] ImportError),

    #[error("frontmatter parse: {0}")]
    Parse(#[from] MemoryParseError),

    #[error("utf-8: {0}")]
    Utf8(#[from] std::str::Utf8Error),

    #[error("{0}")]
    Other(String),
}
