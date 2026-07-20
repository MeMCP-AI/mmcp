//! MCP tool schemas for mmcp.
//!
//! Defines the typed request and response shapes for every tool
//! exposed by the MCP surface. Shared between `mmcp-server` and
//! `mmcp-client` so the two sides cannot drift on wire format.
//!
//! This crate deliberately does not depend on `rmcp` yet. The server
//! and client binaries are the only places where the `rmcp`
//! `#[tool]` attributes land, because keeping the schemas free of
//! framework wiring lets them stay pure data that is easy to test.

pub mod error;
pub mod notes;
pub mod tools;

pub use error::ProtoError;
pub use notes::{Note, NoteLevel};
pub use tools::{
    DiffMemoryRequest, DiffMemoryResponse, GroupInfoRequest, GroupInfoResponse,
    ListMemoriesRequest, ListMemoriesResponse, ListVersionsRequest, ListVersionsResponse,
    MemoryDescriptor, ReadMemoryRequest, ReadMemoryResponse, SearchMemoriesRequest,
    SearchMemoriesResponse, SearchMemoryHit, ToolName, VerifyMemoryRequest, VerifyMemoryResponse,
    VersionEntry, WriteMemoryRequest, WriteMemoryResponse,
};
