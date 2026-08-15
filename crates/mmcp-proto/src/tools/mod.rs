//! Request and response schemas for every MCP tool mmcp exposes.
//!
//! Each tool has one request type and one response type. Types use
//! owned `String` and `Vec` fields so serde round-trips cleanly
//! through JSON and the schemas stay easy to clone into log
//! attributes without lifetime gymnastics.
//!
//! One file per type, or per tightly-coupled request/response pair.
//! [`ToolName`] and [`McpToolId`] are the two tool-identity enums
//! this crate defines; they live in their own `tool_name` and
//! `mcp_tool_id` modules so each enum's doc comment and tests sit
//! next to its declaration, while the parity test binding the two
//! together (`tool_name_and_mcp_tool_id_agree_on_shared_wire_names`,
//! below) stays at this module boundary where the relationship it
//! guards is visible.

mod diff_memory;
mod group_info;
mod list_memories;
mod list_versions;
mod mcp_tool_id;
mod memory_descriptor;
mod read_memory;
mod search_memories;
mod tool_name;
mod verify_memory;
mod write_memory;

pub use diff_memory::{DiffMemoryRequest, DiffMemoryResponse};
pub use group_info::{GroupInfoRequest, GroupInfoResponse};
pub use list_memories::{ListMemoriesRequest, ListMemoriesResponse};
pub use list_versions::{ListVersionsRequest, ListVersionsResponse, VersionEntry};
pub use mcp_tool_id::McpToolId;
pub use memory_descriptor::MemoryDescriptor;
pub use read_memory::{ReadMemoryRequest, ReadMemoryResponse};
pub use search_memories::{SearchMemoriesRequest, SearchMemoriesResponse, SearchMemoryHit};
pub use tool_name::ToolName;
pub use verify_memory::{VerifyMemoryRequest, VerifyMemoryResponse};
pub use write_memory::{WriteMemoryRequest, WriteMemoryResponse};

#[cfg(test)]
mod tests {
    use super::{McpToolId, ToolName};

    /// Regression guard against wire-name drift between the two
    /// independently declared tool enums. `ToolName` backs
    /// `mmcp-server`'s HTTP `/mcp/tool` dispatch; `McpToolId` backs
    /// the full MCP tool registry. Six of `ToolName`'s eight wire
    /// names also exist as `McpToolId` variants (see `ToolName`'s
    /// doc comment for the two that do not); this pins those six to
    /// stay byte-identical so a rename in one enum cannot silently
    /// diverge from the other while both builds stay green.
    #[test]
    fn tool_name_and_mcp_tool_id_agree_on_shared_wire_names() {
        let shared_pairs = [
            (ToolName::ListMemories, McpToolId::ListMemories),
            (ToolName::ReadMemory, McpToolId::ReadMemory),
            (ToolName::WriteMemory, McpToolId::WriteMemory),
            (ToolName::ListVersions, McpToolId::ListVersions),
            (ToolName::SearchMemories, McpToolId::SearchMemories),
            (ToolName::GroupInfo, McpToolId::GroupInfo),
        ];
        for (tool_name, mcp_tool_id) in shared_pairs {
            assert_eq!(
                tool_name.as_str(),
                mcp_tool_id.as_str(),
                "ToolName and McpToolId must agree on the wire name they share",
            );
        }
    }
}
