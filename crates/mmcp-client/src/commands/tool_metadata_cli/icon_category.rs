//! Per-tool icon category and its icon rendering.

use super::defaults::{
    DEBUG_ICON_SRC, FEATURE_ICON_SRC, ISSUE_ICON_SRC, MILESTONE_ICON_SRC, MUTATE_ICON_SRC,
    READ_ICON_SRC, SYNC_ICON_SRC,
};
use super::registry::tool_metadata_for_name;

/// Per-tool category that drives icon selection.
/// Declared per tool in [`super::registry::tool_metadata`], the single exhaustive
/// registry backing icons, `_meta`, and argument risk hints alike;
/// there is no default arm, so a `#[tool]` method without a
/// [`mmcp_proto::McpToolId`] variant and a `tool_metadata` arm fails to compile
/// instead of shipping a generic glyph that misleads operators.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ToolIconCategory {
    /// Read-only tools that walk the local mirror without writing.
    Read,
    /// Local mutators: additive or destructive writes against the
    /// mirror, the sessions store, or `.mmcp.toml`.
    Mutate,
    /// Feature-request tools (`*_feature`).
    Feature,
    /// Issue-tracker tools (`*_issue`), sister to `Feature`.
    Issue,
    /// Milestone tracker tools (`*_milestone`), sister to `Feature`
    /// / `Issue` but a reduced surface.
    Milestone,
    /// `debug_*` raw-git escape hatches.
    Debug,
    /// `sync_*` tools that contact the remote server.
    Sync,
}

/// Resolve a live tool's icon category from its wire name.
pub(crate) fn tool_icon_category(name: &str) -> ToolIconCategory {
    tool_metadata_for_name(name).category
}

/// Build the icon list for a category.
pub(crate) fn icons_for_category(cat: ToolIconCategory) -> Vec<rmcp::model::Icon> {
    let src = match cat {
        ToolIconCategory::Read => READ_ICON_SRC,
        ToolIconCategory::Mutate => MUTATE_ICON_SRC,
        ToolIconCategory::Feature => FEATURE_ICON_SRC,
        ToolIconCategory::Issue => ISSUE_ICON_SRC,
        ToolIconCategory::Milestone => MILESTONE_ICON_SRC,
        ToolIconCategory::Debug => DEBUG_ICON_SRC,
        ToolIconCategory::Sync => SYNC_ICON_SRC,
    };
    vec![rmcp::model::Icon::new(src).with_mime_type("image/svg+xml")]
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    /// Spot-check that category routing covers the obvious
    /// archetypes: one read tool, one debug tool, one sync tool,
    /// one feature tool, one mutate tool, so a future refactor of
    /// the category match can't silently re-bucket entire families.
    #[test]
    fn tool_icon_category_covers_each_archetype() {
        assert_eq!(tool_icon_category("read_memory"), ToolIconCategory::Read);
        assert_eq!(tool_icon_category("write_memory"), ToolIconCategory::Mutate);
        assert_eq!(
            tool_icon_category("read_feature"),
            ToolIconCategory::Feature
        );
        assert_eq!(
            tool_icon_category("debug_read_file"),
            ToolIconCategory::Debug,
        );
        assert_eq!(tool_icon_category("sync_pull"), ToolIconCategory::Sync);
    }

    /// An unmapped tool name panics.
    /// This is the completeness guarantee `tool_metadata` exists for:
    /// a `#[tool]` method that ships without a matching `McpToolId`
    /// variant fails loudly here rather than shipping a misleading icon.
    #[test]
    #[should_panic(expected = "has no McpToolId variant")]
    fn tool_icon_category_rejects_an_unmapped_name() {
        tool_icon_category("future_tool_that_does_not_exist_yet");
    }
}
