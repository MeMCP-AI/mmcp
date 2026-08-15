//! `verify_memory` request/response pair.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerifyMemoryRequest {
    pub memory: Uuid,

    /// Optional note explaining what the caller verified. Stored in
    /// the read-tracking table for audit.
    #[serde(default)]
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerifyMemoryResponse {
    pub memory: Uuid,
    pub verified_at: i64,
}
