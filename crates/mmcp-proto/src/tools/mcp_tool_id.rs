//! [`McpToolId`]: the complete tool enum backing the full
//! `#[tool_router]` surface `mmcp-client`'s `serve` command
//! registers.

use strum::VariantArray as _;
use strum_macros::{IntoStaticStr, VariantArray};

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
///
/// [`VariantArray`] derives [`Self::VARIANTS`] (every variant, in
/// declaration order) and [`IntoStaticStr`] derives the wire-name
/// conversion (`self.into(): &'static str`); both mirror the enum
/// declaration exactly once, the same "no restated mirror" guarantee
/// the previous hand-rolled `define_mcp_tool_id!` macro provided.
/// `#[strum(serialize_all = "snake_case")]` matches the `#[tool]`
/// macro's own snake_case tool-name derivation, so every wire string
/// stays the mechanical snake_case of its variant name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, VariantArray, IntoStaticStr)]
#[strum(serialize_all = "snake_case")]
pub enum McpToolId {
    ListGroups,
    ListMemories,
    ReadMemory,
    ListVersions,
    GroupInfo,
    SearchMemories,
    ReadMemoryBodySections,
    CheckHealth,
    Diagnose,
    DebugReadFile,
    DebugListTree,
    DebugGitLog,
    BootstrapContext,
    Status,
    Version,
    ReadFeature,
    ListFeatures,
    ReadIssue,
    ListIssues,
    ReadMilestone,
    ListMilestones,
    DescribeTools,
    WriteMemory,
    ImportMemory,
    EditMemory,
    EditMemoryBody,
    MoveMemory,
    DebugWriteFile,
    UpdateFeature,
    UpdateIssue,
    UpdateMilestone,
    DeleteMemory,
    InitClaude,
    DeleteFeature,
    DeleteIssue,
    DebugToggle,
    InitProject,
    RenameFeature,
    RenameIssue,
    Subscribe,
    Unsubscribe,
    CreateGroup,
    AddFeature,
    AddIssue,
    AddMilestone,
    ExportArchive,
    ImportArchive,
    SyncFetch,
    SyncPush,
    SyncPull,
    Sync,
}

impl McpToolId {
    /// Canonical snake_case wire name, matching the tool name the
    /// `#[tool]` macro derives from the method it decorates.
    ///
    /// Not `const`: delegates to strum's `IntoStaticStr`-derived
    /// `From<McpToolId> for &'static str`, a genuine trait-dispatch
    /// conversion rather than a `const fn` match. The one call site
    /// that used to need `const` here (`ToolName::as_str`) is no
    /// longer `const` either, since nothing in the workspace
    /// evaluates it at compile time; see that method's doc comment.
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        (*self).into()
    }

    /// Parse a wire tool name into its typed id.
    ///
    /// Returns `None` for a name this enum does not yet know about:
    /// a genuinely new `#[tool]`-registered method needs only a new
    /// variant added above, since this scans [`Self::VARIANTS`]
    /// itself rather than restating the variant list a second time.
    ///
    /// Deliberately hand-written rather than strum's derived
    /// `FromStr`/`EnumString`: this project's parse-fallback
    /// semantics (return `None`, never an error type) are simpler
    /// than what strum's derive produces, and staying hand-written
    /// keeps that contract explicit rather than incidental.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        Self::VARIANTS.iter().copied().find(|id| id.as_str() == s)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn every_mcp_tool_id_round_trips_through_as_str_and_parse() {
        for id in McpToolId::VARIANTS {
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
