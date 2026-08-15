//! Base tool list decoration: icons, `_meta`, output schema.

use super::icon_category::{icons_for_category, tool_icon_category};
use super::output_schema::shared_output_schema;
use super::registry::meta_for_tool;

/// Decorate a base tool list with icons, `_meta`, and output schema.
/// Takes the base list as a parameter rather than fetching it itself.
/// See commands::tools's module doc for why this never imports from commands::serve.
pub(crate) fn decorate_tool_attrs(mut tools: Vec<rmcp::model::Tool>) -> Vec<rmcp::model::Tool> {
    for tool in &mut tools {
        tool.icons = Some(icons_for_category(tool_icon_category(tool.name.as_ref())));
        // Meta lands on the same patching seam as icons so
        // describe_tools and the CLI surface match the live router.
        tool.meta = meta_for_tool(tool.name.as_ref());
        // Every tool gets a permissive object output schema
        // so clients can validate.
        // Per-tool typed schemas are a candidate future refinement.
        tool.output_schema = Some(shared_output_schema());
    }
    tools
}
