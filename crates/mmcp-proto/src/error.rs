//! Error type returned to clients when a tool call cannot be served.

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Failures returned to MCP clients.
///
/// Each variant is serializable so the transport layer can emit them
/// in the error channel without re-wrapping.
#[derive(Debug, Clone, PartialEq, Eq, Error, Serialize, Deserialize)]
#[serde(tag = "kind", content = "message", rename_all = "snake_case")]
pub enum ProtoError {
    /// The caller is not authenticated.
    #[error("not authenticated: {0}")]
    Unauthenticated(String),

    /// The caller is authenticated but not permitted on this memory
    /// or group.
    #[error("forbidden: {0}")]
    Forbidden(String),

    /// The requested memory, group, or version does not exist.
    #[error("not found: {0}")]
    NotFound(String),

    /// A mandatory memory has not been read this session. The
    /// message lists the outstanding memories.
    #[error("mandatory memories unread: {0}")]
    MandatoryUnread(String),

    /// Request payload did not match the schema.
    #[error("invalid request: {0}")]
    InvalidRequest(String),

    /// Server-side failure. Contains a short description suitable
    /// for display; the full error is logged server-side.
    #[error("internal error: {0}")]
    Internal(String),
}
