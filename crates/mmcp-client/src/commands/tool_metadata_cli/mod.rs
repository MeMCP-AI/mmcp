//! Shared per-tool metadata registry: icon category, `_meta`
//! advisory keys, and argument risk hints.
//!
//! `commands::serve` (both the live `#[tool_router]` patching inside
//! `McpServer::new` and the `describe_tools` MCP tool) and
//! `commands::tools` (the `mmcp tools` CLI, via `main.rs` as its
//! composition root) both decorate the same canonical tool list with
//! the same icons / `_meta` / risk hints. The registry is pure data
//! over [`mmcp_proto::McpToolId`] and `rmcp::model::*` types, with no dependency
//! on `McpServer` itself, so it lives here instead of in
//! `commands::serve`, and [`decorate_tool_attrs`] is the shared
//! decoration step both callers apply to their own base tool list.
//!
//! `McpServer::registered_tool_attrs()` (the live, macro-derived base list) stays in
//! `commands::serve`: it reads `Self::tool_router().map`, which only exists on `McpServer`'s own
//! `#[tool_router]` impl block.
//! See commands::tools's module doc for why this never imports from commands::serve.

mod arg_risk_hint;
mod decorate;
mod defaults;
mod icon_category;
mod output_schema;
mod registry;

pub(crate) use arg_risk_hint::{ArgRiskHint, arg_risk_hints_for};
pub(crate) use decorate::decorate_tool_attrs;
pub(crate) use icon_category::{icons_for_category, tool_icon_category};
pub(crate) use output_schema::shared_output_schema;
pub(crate) use registry::meta_for_tool;
