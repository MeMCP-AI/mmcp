//! Request and response schemas for every MCP tool mmcp exposes.
//!
//! Each tool has one request type and one response type. Types use
//! owned `String` and `Vec` fields so serde round-trips cleanly
//! through JSON and the schemas stay easy to clone into log
//! attributes without lifetime gymnastics.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::warnings::Warning;

/// Enumeration of every MCP tool mmcp exposes.
///
/// Kept as a small enum so logging, metrics, and authorization
/// middleware can branch on the tool being invoked without parsing
/// strings. New tools must be added here and to the server and
/// client dispatchers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolName {
    ListMemories,
    ReadMemory,
    WriteMemory,
    VerifyMemory,
    ListVersions,
    DiffMemory,
    SearchMemories,
    GroupInfo,
}

impl ToolName {
    /// Canonical string form exposed on the MCP wire.
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            ToolName::ListMemories => "list_memories",
            ToolName::ReadMemory => "read_memory",
            ToolName::WriteMemory => "write_memory",
            ToolName::VerifyMemory => "verify_memory",
            ToolName::ListVersions => "list_versions",
            ToolName::DiffMemory => "diff_memory",
            ToolName::SearchMemories => "search_memories",
            ToolName::GroupInfo => "group_info",
        }
    }
}

/// Short summary of a memory used in listings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryDescriptor {
    pub id: Uuid,
    pub group: Uuid,
    pub slug: String,
    pub name: String,
    pub description: String,
    pub kind: String,
    pub mandatory: bool,
    pub latest_version: Option<String>,
}

// ---- list_memories --------------------------------------------------

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

// ---- read_memory ----------------------------------------------------

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
    pub warnings: Vec<Warning>,
}

// ---- write_memory ---------------------------------------------------

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

// ---- verify_memory --------------------------------------------------

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

// ---- list_versions --------------------------------------------------

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

// ---- diff_memory ----------------------------------------------------

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

// ---- search_memories ------------------------------------------------

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

// ---- group_info -----------------------------------------------------

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_name_serializes_to_snake_case() {
        let s = serde_json::to_string(&ToolName::WriteMemory).unwrap();
        assert_eq!(s, "\"write_memory\"");
    }

    #[test]
    fn tool_name_as_str_matches_serde() {
        for tool in [
            ToolName::ListMemories,
            ToolName::ReadMemory,
            ToolName::WriteMemory,
            ToolName::VerifyMemory,
            ToolName::ListVersions,
            ToolName::DiffMemory,
            ToolName::SearchMemories,
            ToolName::GroupInfo,
        ] {
            let quoted = format!("\"{}\"", tool.as_str());
            assert_eq!(serde_json::to_string(&tool).unwrap(), quoted);
        }
    }

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

    #[test]
    fn read_response_preserves_warnings() {
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
            warnings: vec![crate::warnings::Warning::new(
                crate::warnings::WarningKind::FirstReadThisSession,
                "first read this session",
            )],
        };
        let json = serde_json::to_string(&res).unwrap();
        let back: ReadMemoryResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(back, res);
    }
}
