//! Request and response schemas for every MCP tool mmcp exposes.
//!
//! Each tool has one request type and one response type. Types use
//! owned `String` and `Vec` fields so serde round-trips cleanly
//! through JSON and the schemas stay easy to clone into log
//! attributes without lifetime gymnastics.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::notes::Note;

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

/// Identifies one tool on the full `#[tool_router]` surface that
/// `mmcp-client`'s `serve` command registers.
///
/// Distinct from [`ToolName`]: that enum covers only the narrow
/// subset `mmcp-server`'s HTTP `/mcp/tool` route dispatches on
/// directly. This enum spans the complete tool surface so per-tool
/// metadata (icon category, `_meta` advisory keys, argument risk
/// hints) can be declared through one exhaustive match keyed on a
/// typed variant, instead of parallel string-keyed side tables that
/// each need their own edit when a tool is added.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum McpToolId {
    ListGroups,
    ListMemories,
    ReadMemory,
    ListVersions,
    GroupInfo,
    SearchMemories,
    ReadMemoryBodySections,
    CheckHealth,
    Diagnose,
    DebugReadFile,
    DebugListTree,
    DebugGitLog,
    BootstrapContext,
    Status,
    Version,
    ReadFeature,
    ListFeatures,
    ReadIssue,
    ListIssues,
    ReadMilestone,
    ListMilestones,
    DescribeTools,
    WriteMemory,
    ImportMemory,
    EditMemory,
    EditMemoryBody,
    MoveMemory,
    DebugWriteFile,
    UpdateFeature,
    UpdateIssue,
    UpdateMilestone,
    DeleteMemory,
    InitClaude,
    DeleteFeature,
    DeleteIssue,
    DebugToggle,
    InitProject,
    RenameFeature,
    RenameIssue,
    Subscribe,
    Unsubscribe,
    CreateGroup,
    AddFeature,
    AddIssue,
    AddMilestone,
    ExportArchive,
    ImportArchive,
    SyncFetch,
    SyncPush,
    SyncPull,
    Sync,
}

impl McpToolId {
    /// Every variant, in declaration order.
    ///
    /// `every_mcp_tool_id_round_trips_through_as_str_and_parse`
    /// exercises every entry here against [`Self::as_str`] and
    /// [`Self::parse`]; it does not detect a variant left out of
    /// this list entirely, only a listed entry that fails to
    /// round-trip.
    pub const ALL: &'static [McpToolId] = &[
        Self::ListGroups,
        Self::ListMemories,
        Self::ReadMemory,
        Self::ListVersions,
        Self::GroupInfo,
        Self::SearchMemories,
        Self::ReadMemoryBodySections,
        Self::CheckHealth,
        Self::Diagnose,
        Self::DebugReadFile,
        Self::DebugListTree,
        Self::DebugGitLog,
        Self::BootstrapContext,
        Self::Status,
        Self::Version,
        Self::ReadFeature,
        Self::ListFeatures,
        Self::ReadIssue,
        Self::ListIssues,
        Self::ReadMilestone,
        Self::ListMilestones,
        Self::DescribeTools,
        Self::WriteMemory,
        Self::ImportMemory,
        Self::EditMemory,
        Self::EditMemoryBody,
        Self::MoveMemory,
        Self::DebugWriteFile,
        Self::UpdateFeature,
        Self::UpdateIssue,
        Self::UpdateMilestone,
        Self::DeleteMemory,
        Self::InitClaude,
        Self::DeleteFeature,
        Self::DeleteIssue,
        Self::DebugToggle,
        Self::InitProject,
        Self::RenameFeature,
        Self::RenameIssue,
        Self::Subscribe,
        Self::Unsubscribe,
        Self::CreateGroup,
        Self::AddFeature,
        Self::AddIssue,
        Self::AddMilestone,
        Self::ExportArchive,
        Self::ImportArchive,
        Self::SyncFetch,
        Self::SyncPush,
        Self::SyncPull,
        Self::Sync,
    ];

    /// Canonical snake_case wire name, matching the tool name the
    /// `#[tool]` macro derives from the method it decorates.
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::ListGroups => "list_groups",
            Self::ListMemories => "list_memories",
            Self::ReadMemory => "read_memory",
            Self::ListVersions => "list_versions",
            Self::GroupInfo => "group_info",
            Self::SearchMemories => "search_memories",
            Self::ReadMemoryBodySections => "read_memory_body_sections",
            Self::CheckHealth => "check_health",
            Self::Diagnose => "diagnose",
            Self::DebugReadFile => "debug_read_file",
            Self::DebugListTree => "debug_list_tree",
            Self::DebugGitLog => "debug_git_log",
            Self::BootstrapContext => "bootstrap_context",
            Self::Status => "status",
            Self::Version => "version",
            Self::ReadFeature => "read_feature",
            Self::ListFeatures => "list_features",
            Self::ReadIssue => "read_issue",
            Self::ListIssues => "list_issues",
            Self::ReadMilestone => "read_milestone",
            Self::ListMilestones => "list_milestones",
            Self::DescribeTools => "describe_tools",
            Self::WriteMemory => "write_memory",
            Self::ImportMemory => "import_memory",
            Self::EditMemory => "edit_memory",
            Self::EditMemoryBody => "edit_memory_body",
            Self::MoveMemory => "move_memory",
            Self::DebugWriteFile => "debug_write_file",
            Self::UpdateFeature => "update_feature",
            Self::UpdateIssue => "update_issue",
            Self::UpdateMilestone => "update_milestone",
            Self::DeleteMemory => "delete_memory",
            Self::InitClaude => "init_claude",
            Self::DeleteFeature => "delete_feature",
            Self::DeleteIssue => "delete_issue",
            Self::DebugToggle => "debug_toggle",
            Self::InitProject => "init_project",
            Self::RenameFeature => "rename_feature",
            Self::RenameIssue => "rename_issue",
            Self::Subscribe => "subscribe",
            Self::Unsubscribe => "unsubscribe",
            Self::CreateGroup => "create_group",
            Self::AddFeature => "add_feature",
            Self::AddIssue => "add_issue",
            Self::AddMilestone => "add_milestone",
            Self::ExportArchive => "export_archive",
            Self::ImportArchive => "import_archive",
            Self::SyncFetch => "sync_fetch",
            Self::SyncPush => "sync_push",
            Self::SyncPull => "sync_pull",
            Self::Sync => "sync",
        }
    }

    /// Parse a wire tool name into its typed id.
    ///
    /// Returns `None` for a name this enum does not yet know about:
    /// a genuinely new `#[tool]`-registered method needs both a new
    /// variant and an arm here before its name resolves.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "list_groups" => Self::ListGroups,
            "list_memories" => Self::ListMemories,
            "read_memory" => Self::ReadMemory,
            "list_versions" => Self::ListVersions,
            "group_info" => Self::GroupInfo,
            "search_memories" => Self::SearchMemories,
            "read_memory_body_sections" => Self::ReadMemoryBodySections,
            "check_health" => Self::CheckHealth,
            "diagnose" => Self::Diagnose,
            "debug_read_file" => Self::DebugReadFile,
            "debug_list_tree" => Self::DebugListTree,
            "debug_git_log" => Self::DebugGitLog,
            "bootstrap_context" => Self::BootstrapContext,
            "status" => Self::Status,
            "version" => Self::Version,
            "read_feature" => Self::ReadFeature,
            "list_features" => Self::ListFeatures,
            "read_issue" => Self::ReadIssue,
            "list_issues" => Self::ListIssues,
            "read_milestone" => Self::ReadMilestone,
            "list_milestones" => Self::ListMilestones,
            "describe_tools" => Self::DescribeTools,
            "write_memory" => Self::WriteMemory,
            "import_memory" => Self::ImportMemory,
            "edit_memory" => Self::EditMemory,
            "edit_memory_body" => Self::EditMemoryBody,
            "move_memory" => Self::MoveMemory,
            "debug_write_file" => Self::DebugWriteFile,
            "update_feature" => Self::UpdateFeature,
            "update_issue" => Self::UpdateIssue,
            "update_milestone" => Self::UpdateMilestone,
            "delete_memory" => Self::DeleteMemory,
            "init_claude" => Self::InitClaude,
            "delete_feature" => Self::DeleteFeature,
            "delete_issue" => Self::DeleteIssue,
            "debug_toggle" => Self::DebugToggle,
            "init_project" => Self::InitProject,
            "rename_feature" => Self::RenameFeature,
            "rename_issue" => Self::RenameIssue,
            "subscribe" => Self::Subscribe,
            "unsubscribe" => Self::Unsubscribe,
            "create_group" => Self::CreateGroup,
            "add_feature" => Self::AddFeature,
            "add_issue" => Self::AddIssue,
            "add_milestone" => Self::AddMilestone,
            "export_archive" => Self::ExportArchive,
            "import_archive" => Self::ImportArchive,
            "sync_fetch" => Self::SyncFetch,
            "sync_push" => Self::SyncPush,
            "sync_pull" => Self::SyncPull,
            "sync" => Self::Sync,
            _ => return None,
        })
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
    /// Notes channel.
    /// Entries surface session-state signals (`first_read_this_session`, `stale_by_kind`,
    /// …) and any frontmatter-parse warnings observed while rendering this response.
    /// Absent / empty in the common case.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<Note>,
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
    fn every_mcp_tool_id_round_trips_through_as_str_and_parse() {
        for id in McpToolId::ALL {
            let parsed = McpToolId::parse(id.as_str());
            assert_eq!(
                parsed,
                Some(*id),
                "{} must parse back to itself",
                id.as_str(),
            );
        }
    }

    #[test]
    fn mcp_tool_id_parse_rejects_unknown_name() {
        assert_eq!(
            McpToolId::parse("future_tool_that_does_not_exist_yet"),
            None
        );
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
            notes: vec![crate::notes::Note::warn(
                "first_read_this_session",
                "first read this session",
            )],
        };
        let json = serde_json::to_string(&res).unwrap();
        let back: ReadMemoryResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(back, res);
    }
}
