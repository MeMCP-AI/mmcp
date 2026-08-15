//! [`ToolName`]: the narrow tool enum `mmcp-server`'s HTTP `/mcp/tool`
//! route dispatches on directly.

use serde::{Deserialize, Serialize};

/// Enumeration of every MCP tool mmcp exposes.
///
/// Kept as a small enum so logging, metrics, and authorization
/// middleware can branch on the tool being invoked without parsing
/// strings. New tools must be added here and to the server and
/// client dispatchers.
///
/// Six of these eight wire names (every one except `verify_memory`
/// and `diff_memory`) also appear as [`super::McpToolId`] variants:
/// those two tools are dispatched only through `mmcp-server`'s HTTP
/// `/mcp/tool` route, never registered on the full `#[tool_router]`
/// surface `McpToolId` models, so no `McpToolId` counterpart exists
/// for them and a total `From<ToolName> for McpToolId` conversion is
/// not possible. Nothing here ties the two enums together
/// structurally, so a renamed wire string in one would silently
/// diverge from the other; `tool_name_and_mcp_tool_id_agree_on_shared_wire_names`
/// in `tools/mod.rs` is a parity test over the six names both enums
/// restate, smaller and safer than inventing two `McpToolId` variants
/// for tools that do not belong on that surface.
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
}
