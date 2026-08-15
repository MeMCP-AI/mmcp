//! `list_versions` request/response pair, plus the `VersionEntry`
//! type its response is built from.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListVersionsRequest {
    pub memory: Uuid,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VersionEntry {
    pub version: String,
    pub commit: String,
    pub author: String,
    pub published_at: i64,
    pub summary: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListVersionsResponse {
    pub memory: Uuid,
    pub versions: Vec<VersionEntry>,
}
