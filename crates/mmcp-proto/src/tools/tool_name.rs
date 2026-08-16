//! [`ToolName`]: the narrow tool enum `mmcp-server`'s HTTP `/mcp/tool`
//! route dispatches on directly.

use serde::{Deserialize, Serialize};

use super::McpToolId;

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
/// not possible. [`Self::as_str`] delegates each of the six shared
/// names to [`McpToolId::as_str`] instead of restating its own
/// literal, so the two enums cannot drift on the names they share;
/// only `verify_memory` and `diff_memory` carry their own literal,
/// since no `McpToolId` counterpart exists to delegate to.
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
    ///
    /// Not `const`: the six delegated arms call
    /// [`McpToolId::as_str`], which itself delegates to strum's
    /// `IntoStaticStr`-derived, non-`const` `From<McpToolId> for
    /// &'static str` conversion (no call site anywhere in the
    /// workspace evaluates `ToolName::as_str` in a const context, so
    /// this is not a behavior change for any real caller).
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            ToolName::ListMemories => McpToolId::ListMemories.as_str(),
            ToolName::ReadMemory => McpToolId::ReadMemory.as_str(),
            ToolName::WriteMemory => McpToolId::WriteMemory.as_str(),
            ToolName::VerifyMemory => "verify_memory",
            ToolName::ListVersions => McpToolId::ListVersions.as_str(),
            ToolName::DiffMemory => "diff_memory",
            ToolName::SearchMemories => McpToolId::SearchMemories.as_str(),
            ToolName::GroupInfo => McpToolId::GroupInfo.as_str(),
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
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
