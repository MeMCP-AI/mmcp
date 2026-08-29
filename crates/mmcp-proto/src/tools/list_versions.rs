//! `list_versions` request/response pair, plus the `VersionEntry`
//! type its response is built from.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListVersionsRequest {
    pub memory: Uuid,

    /// Cap the number of returned rows. `None` (the default, and
    /// what every request omitting the field deserializes to)
    /// preserves the historical unbounded response; a caller opts
    /// into pagination explicitly by setting this.
    #[serde(default)]
    pub limit: Option<u64>,
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
