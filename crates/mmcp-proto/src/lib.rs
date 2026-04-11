//! MCP tool schemas for mmcp.
//!
//! Defines the typed request and response shapes for every tool exposed
//! by the MCP surface (`list_memories`, `read_memory`, `write_memory`,
//! `verify_memory`, `list_versions`, `diff_memory`, `search_memories`,
//! `group_info`). Shared between `mmcp-server` and `mmcp-client` so the
//! two sides cannot drift.
//!
//! Built on top of the `rmcp` crate, the official Rust MCP SDK.
