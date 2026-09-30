//! [`InitClaudeAction`], the action of the `init_claude` MCP tool.

use rmcp::schemars::JsonSchema;
use serde::Deserialize;

/// Action for `init_claude`. Matches the CLI's mutually-exclusive flag
/// set (override / append / convert).
#[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
#[serde(rename_all = "snake_case")]
pub enum InitClaudeAction {
    /// Replace CLAUDE.md with a fresh mmcp stub.
    Override,
    /// Insert or replace the mmcp-managed fence inside CLAUDE.md.
    Append,
    /// Split CLAUDE.md into typed memories, then replace with stub.
    Convert,
}

/// Map `InitClaudeAction` onto the wire string used in tool responses.
pub fn action_wire(action: InitClaudeAction) -> &'static str {
    match action {
        InitClaudeAction::Override => "override",
        InitClaudeAction::Append => "append",
        InitClaudeAction::Convert => "convert",
    }
}
