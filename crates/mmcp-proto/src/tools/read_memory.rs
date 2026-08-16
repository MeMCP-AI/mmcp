//! `read_memory` request/response pair.

use serde::{Deserialize, Serialize};

use super::MemoryDescriptor;
use crate::notes::Note;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadMemoryRequest {
    /// Memory identifier or the `group/slug` string form.
    pub target: String,

    /// Optional version pin. `None` means `latest`.
    #[serde(default)]
    pub version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadMemoryResponse {
    pub descriptor: MemoryDescriptor,
    pub version: String,
    pub body: String,
    /// Notes channel.
    /// Entries surface session-state signals (`first_read_this_session`, `stale_by_kind`,
    /// …) and any frontmatter-parse warnings observed while rendering this response.
    /// Absent / empty in the common case.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<Note>,
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use uuid::Uuid;

    #[test]
    fn read_response_preserves_notes() {
        let res = ReadMemoryResponse {
            descriptor: MemoryDescriptor {
                id: Uuid::now_v7(),
                group: Uuid::now_v7(),
                slug: "rules".into(),
                name: "Rules".into(),
                description: "d".into(),
                kind: "rule".into(),
                mandatory: true,
                latest_version: Some("1.0.0".into()),
            },
            version: "1.0.0".into(),
            body: "# Body\n".into(),
            notes: vec![Note::warn(
                "first_read_this_session",
                "first read this session",
            )],
        };
        let json = serde_json::to_string(&res).unwrap();
        let back: ReadMemoryResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(back, res);
    }
}
