//! `search_memories` request/response pair, plus the `SearchMemoryHit`
//! type its response is built from.

use serde::{Deserialize, Serialize};

use super::MemoryDescriptor;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchMemoriesRequest {
    pub query: String,
    #[serde(default)]
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchMemoryHit {
    pub descriptor: MemoryDescriptor,
    pub score: f32,
    pub snippet: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchMemoriesResponse {
    pub hits: Vec<SearchMemoryHit>,
}
