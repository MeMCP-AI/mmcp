//! `write_memory` request/response pair.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::MemoryDescriptor;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WriteMemoryRequest {
    /// Group the memory lives in or should be created in.
    pub group: Uuid,

    /// Memory slug inside the group repo.
    pub slug: String,

    /// Full raw Markdown source, including the TOML frontmatter.
    pub content: String,

    /// Commit message explaining the edit.
    pub message: String,

    /// Requested bump level. Defaults to `minor` on the server side
    /// when omitted.
    #[serde(default)]
    pub bump: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WriteMemoryResponse {
    pub descriptor: MemoryDescriptor,
    /// `None` when the edit was applied locally and is pending push
    /// (the server has not yet assigned a version).
    pub assigned_version: Option<String>,
    pub commit: String,
}
