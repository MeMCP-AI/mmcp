//! `mmcp tools` implementation.
//!
//! Operator-facing inspection of the MCP tool surface: prints one
//! row per registered tool with the annotation matrix
//! (read_only / destructive / idempotent / open_world) plus a short
//! description. Same data the `describe_tools` MCP tool returns,
//! surfaced through the CLI for hook scripts and ad-hoc
//! audits that should not need to start the stdio server.
//!
//! This module never imports from `commands::serve`: the canonical,
//! live tool list only `McpServer::tool_router()` can produce (in
//! `commands::serve`), so `main.rs`, the binary's composition root,
//! fetches it via `commands::serve::registered_tool_attrs()` and
//! passes it into [`run`]. `commands::tools` only ever renders a
//! list it is handed.

use anyhow::Result;
use clap::ValueEnum;
use serde::Serialize;

use crate::commands::tool_metadata_cli::{ArgRiskHint, arg_risk_hints_for};

/// Output format selector for `mmcp tools`.
#[derive(Clone, Copy, Debug, Default, ValueEnum)]
pub enum ToolsFormat {
    /// Pretty ASCII table, default for interactive operators.
    #[default]
    Table,
    /// Pretty-printed JSON for scripts piping the surface into jq.
    Json,
}

/// Single-row projection matching the `describe_tools` wire shape so
/// the CLI JSON output stays byte-comparable to the MCP response
/// (modulo the wrapping object).
#[derive(Serialize)]
pub(crate) struct ToolRow {
    pub(crate) name: String,
    pub(crate) title: Option<String>,
    pub(crate) description: String,
    pub(crate) read_only: Option<bool>,
    pub(crate) destructive: Option<bool>,
    pub(crate) idempotent: Option<bool>,
    pub(crate) open_world: Option<bool>,
    pub(crate) arg_risk_hints: &'static [ArgRiskHint],
}

/// Project a decorated tool list (already carrying icons / `_meta` /
/// output schema from `tool_metadata_cli::decorate_tool_attrs`, via
/// whichever base list the caller supplied) into the CLI/JSON row
/// shape.
pub(crate) fn build_rows(tools: Vec<rmcp::model::Tool>) -> Vec<ToolRow> {
    tools
        .into_iter()
        .map(|tool| {
            let ann = tool.annotations.as_ref();
            let arg_risk_hints = arg_risk_hints_for(tool.name.as_ref());
            ToolRow {
                name: tool.name.to_string(),
                title: ann.and_then(|a| a.title.clone()),
                description: tool
                    .description
                    .as_ref()
                    .map(|c| c.to_string())
                    .unwrap_or_default(),
                read_only: ann.and_then(|a| a.read_only_hint),
                destructive: ann.and_then(|a| a.destructive_hint),
                idempotent: ann.and_then(|a| a.idempotent_hint),
                open_world: ann.and_then(|a| a.open_world_hint),
                arg_risk_hints,
            }
        })
        .collect()
}

/// Print the tool catalogue to stdout in the requested format.
///
/// `tools` is the caller's own base list (see the module doc: only
/// `main.rs` can supply the live, macro-derived one).
pub fn run(format: ToolsFormat, tools: Vec<rmcp::model::Tool>) -> Result<()> {
    let rows = build_rows(tools);

    match format {
        ToolsFormat::Json => {
            let payload = serde_json::json!({
                "count": rows.len(),
                "tools": rows,
            });
            println!("{}", serde_json::to_string_pretty(&payload)?);
        }
        ToolsFormat::Table => {
            print_table(&rows);
        }
    }
    Ok(())
}

/// Hand-rolled column formatter: no new dep just for one CLI table.
/// Booleans render as `Y` / `-` (unset); fields stay ASCII so the
/// output looks the same on Windows terminals as on Unix.
fn print_table(rows: &[ToolRow]) {
    // Six narrow flag columns plus name, title, description.
    let header_name = "NAME";
    let header_title = "TITLE";
    let header_ro = "RO";
    let header_de = "DE";
    let header_id = "ID";
    let header_ow = "OW";
    let header_desc = "DESCRIPTION";

    let name_w = rows
        .iter()
        .map(|r| r.name.len())
        .max()
        .unwrap_or(0)
        .max(header_name.len());
    let title_w = rows
        .iter()
        .map(|r| r.title.as_deref().unwrap_or("").len())
        .max()
        .unwrap_or(0)
        .max(header_title.len());

    println!(
        "{:<name$}  {:<title$}  {:>2}  {:>2}  {:>2}  {:>2}  {}",
        header_name,
        header_title,
        header_ro,
        header_de,
        header_id,
        header_ow,
        header_desc,
        name = name_w,
        title = title_w,
    );

    for row in rows {
        println!(
            "{:<name$}  {:<title$}  {:>2}  {:>2}  {:>2}  {:>2}  {}",
            row.name,
            row.title.as_deref().unwrap_or(""),
            bool_cell(row.read_only),
            bool_cell(row.destructive),
            bool_cell(row.idempotent),
            bool_cell(row.open_world),
            row.description,
            name = name_w,
            title = title_w,
        );
    }
}

fn bool_cell(value: Option<bool>) -> &'static str {
    match value {
        Some(true) => "Y",
        Some(false) => "n",
        None => "-",
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    /// `build_rows` on a synthetic list still wires up the
    /// arg-risk-hint column: real registry coverage against
    /// `read_memory` / `write_memory` lives in
    /// `main::tests::tools_json_lists_every_registered_tool_with_hint_columns`,
    /// the only place in this binary that legitimately holds both
    /// `commands::tools` and `commands::serve`'s live tool list.
    #[test]
    fn build_rows_wires_up_arg_risk_hints_by_tool_name() {
        let tool = rmcp::model::Tool::new(
            std::borrow::Cow::Borrowed("write_memory"),
            std::borrow::Cow::Borrowed("desc"),
            std::sync::Arc::new(serde_json::Map::new()),
        );
        let rows = build_rows(vec![tool]);
        assert_eq!(rows.len(), 1);
        assert!(
            rows[0].arg_risk_hints.iter().any(|h| h.arg == "override"),
            "write_memory must surface an `override` arg_risk_hint",
        );
    }

    #[test]
    fn bool_cell_renders_three_states() {
        assert_eq!(bool_cell(Some(true)), "Y");
        assert_eq!(bool_cell(Some(false)), "n");
        assert_eq!(bool_cell(None), "-");
    }
}
