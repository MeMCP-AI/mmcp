//! `list_memories` request/response pair.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::MemoryDescriptor;

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ListMemoriesRequest {
    /// Restrict to a specific group. `None` lists memories across
    /// the whole effective load set.
    #[serde(default)]
    pub group: Option<Uuid>,

    /// Only return memories with one of the given kinds. Empty means
    /// no filter.
    #[serde(default)]
    pub kinds: Vec<String>,

    /// Only return memories marked mandatory if `true`. `None`
    /// disables the filter entirely.
    #[serde(default)]
    pub only_mandatory: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListMemoriesResponse {
    pub memories: Vec<MemoryDescriptor>,
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn list_request_round_trips_through_json() {
        let req = ListMemoriesRequest {
            group: Some(Uuid::now_v7()),
            kinds: vec!["rule".into(), "reference".into()],
            only_mandatory: Some(true),
        };
        let json = serde_json::to_string(&req).unwrap();
        let parsed: ListMemoriesRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, req);
    }
}
