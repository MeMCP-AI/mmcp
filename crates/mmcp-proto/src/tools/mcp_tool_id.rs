//! [`McpToolId`]: the complete tool enum backing the full
//! `#[tool_router]` surface `mmcp-client`'s `serve` command
//! registers.

/// Identifies one tool on the full `#[tool_router]` surface that
/// `mmcp-client`'s `serve` command registers.
///
/// Distinct from [`super::ToolName`]: that enum covers only the
/// narrow subset `mmcp-server`'s HTTP `/mcp/tool` route dispatches on
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
    /// (below) exercises every entry here against [`Self::as_str`]
    /// and [`Self::parse`]; it does not detect a variant left out of
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
