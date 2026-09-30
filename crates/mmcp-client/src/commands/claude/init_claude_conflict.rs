//! [`InitClaudeConflict`], the pre-supplied conflict answer of the `init_claude` MCP tool.

use rmcp::schemars::JsonSchema;
use serde::Deserialize;

/// Pre-supplied answer to the dirty-file conflict question. Callers
/// who know they want to override / backup+override / cancel up front
/// set this on the first call; otherwise the tool returns a
/// `conflict_unresolved` error listing the required follow-up.
///
/// This is the serializable counterpart to the CLI's interactive
/// prompt. Future rmcp releases that expose `ElicitationRequest` can
/// replace the error-then-retry contract with a synchronous prompt;
/// the argument's shape stays the same.
#[derive(Debug, Clone, Copy, Deserialize, serde::Serialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
#[serde(rename_all = "snake_case")]
pub enum InitClaudeConflict {
    /// Overwrite the existing file without writing a .bak copy.
    Override,
    /// Write CLAUDE.md.bak, then overwrite.
    BackupOverride,
    /// Abort; do not touch CLAUDE.md.
    Cancel,
}
