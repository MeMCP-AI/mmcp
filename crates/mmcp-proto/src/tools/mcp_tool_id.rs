//! [`McpToolId`]: the complete tool enum backing the full
//! `#[tool_router]` surface `mmcp-client`'s `serve` command
//! registers.

/// Declares the full [`McpToolId`] variant list exactly once.
///
/// Mirrors `mmcp-core`'s `define_memory_kind!`: every mirror of the
/// variant list (the enum itself, [`McpToolId::ALL`],
/// [`McpToolId::as_str`], and [`McpToolId::parse`]) expands from
/// this one invocation, so a variant can never update one mirror
/// while leaving another behind. `parse` is generated from `ALL`
/// itself (a linear scan over [`McpToolId::as_str`]) rather than its
/// own restated match, so a variant left out of `ALL` cannot silently
/// parse anyway while failing every other mirror's coverage.
macro_rules! define_mcp_tool_id {
    ($($variant:ident => $wire:literal),+ $(,)?) => {
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
            $($variant,)+
        }

        impl McpToolId {
            /// Every variant, in declaration order.
            pub const ALL: &'static [McpToolId] = &[$(Self::$variant),+];

            /// Canonical snake_case wire name, matching the tool name the
            /// `#[tool]` macro derives from the method it decorates.
            #[must_use]
            pub const fn as_str(&self) -> &'static str {
                match self {
                    $(Self::$variant => $wire,)+
                }
            }
        }
    };
}

define_mcp_tool_id! {
    ListGroups => "list_groups",
    ListMemories => "list_memories",
    ReadMemory => "read_memory",
    ListVersions => "list_versions",
    GroupInfo => "group_info",
    SearchMemories => "search_memories",
    ReadMemoryBodySections => "read_memory_body_sections",
    CheckHealth => "check_health",
    Diagnose => "diagnose",
    DebugReadFile => "debug_read_file",
    DebugListTree => "debug_list_tree",
    DebugGitLog => "debug_git_log",
    BootstrapContext => "bootstrap_context",
    Status => "status",
    Version => "version",
    ReadFeature => "read_feature",
    ListFeatures => "list_features",
    ReadIssue => "read_issue",
    ListIssues => "list_issues",
    ReadMilestone => "read_milestone",
    ListMilestones => "list_milestones",
    DescribeTools => "describe_tools",
    WriteMemory => "write_memory",
    ImportMemory => "import_memory",
    EditMemory => "edit_memory",
    EditMemoryBody => "edit_memory_body",
    MoveMemory => "move_memory",
    DebugWriteFile => "debug_write_file",
    UpdateFeature => "update_feature",
    UpdateIssue => "update_issue",
    UpdateMilestone => "update_milestone",
    DeleteMemory => "delete_memory",
    InitClaude => "init_claude",
    DeleteFeature => "delete_feature",
    DeleteIssue => "delete_issue",
    DebugToggle => "debug_toggle",
    InitProject => "init_project",
    RenameFeature => "rename_feature",
    RenameIssue => "rename_issue",
    Subscribe => "subscribe",
    Unsubscribe => "unsubscribe",
    CreateGroup => "create_group",
    AddFeature => "add_feature",
    AddIssue => "add_issue",
    AddMilestone => "add_milestone",
    ExportArchive => "export_archive",
    ImportArchive => "import_archive",
    SyncFetch => "sync_fetch",
    SyncPush => "sync_push",
    SyncPull => "sync_pull",
    Sync => "sync",
}

impl McpToolId {
    /// Parse a wire tool name into its typed id.
    ///
    /// Returns `None` for a name this enum does not yet know about:
    /// a genuinely new `#[tool]`-registered method needs both a new
    /// variant (added to the [`define_mcp_tool_id!`] invocation
    /// above) and nothing else here, since this scans [`Self::ALL`]
    /// itself rather than restating the variant list a second time.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|id| id.as_str() == s)
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
