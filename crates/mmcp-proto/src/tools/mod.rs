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
//! next to its declaration. `ToolName::as_str` delegates each wire
//! name it shares with `McpToolId` to `McpToolId::as_str`, so the
//! two enums cannot drift on those names; no parity test is needed
//! to bind them together.

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
