//! `group_info` request/response pair.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GroupInfoRequest {
    pub group: Uuid,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GroupInfoResponse {
    pub id: Uuid,
    pub slug: String,
    pub owner: String,
    pub display_name: Option<String>,
    pub memory_count: u32,
    pub effective_role: String,
}
