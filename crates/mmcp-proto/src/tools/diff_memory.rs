//! `diff_memory` request/response pair.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffMemoryRequest {
    pub memory: Uuid,
    pub from_version: String,
    pub to_version: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffMemoryResponse {
    pub memory: Uuid,
    pub from_version: String,
    pub to_version: String,
    /// Unified diff text.
    pub diff: String,
}
