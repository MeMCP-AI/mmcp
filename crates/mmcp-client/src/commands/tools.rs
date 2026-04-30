//! `mmcp tools` implementation.
//!
//! Operator-facing inspection of the MCP tool surface: prints one
//! row per registered tool with the FR-029 annotation matrix
//! (read_only / destructive / idempotent / open_world) plus a short
//! description. Same data the `describe_tools` MCP tool returns
//! (FR-031), surfaced through the CLI for hook scripts and ad-hoc
//! audits that should not need to start the stdio server.

use anyhow::Result;
use clap::ValueEnum;
use serde::Serialize;

use crate::commands::serve::registered_tool_attrs;

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
struct ToolRow {
    name: String,
    title: Option<String>,
    description: String,
    read_only: Option<bool>,
    destructive: Option<bool>,
    idempotent: Option<bool>,
    open_world: Option<bool>,
}

/// Print the tool catalogue to stdout in the requested format.
pub fn run(format: ToolsFormat) -> Result<()> {
    let rows: Vec<ToolRow> = registered_tool_attrs()
        .into_iter()
        .map(|tool| {
            let ann = tool.annotations.as_ref();
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
            }
        })
        .collect();

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

/// Hand-rolled column formatter — no new dep just for one CLI table.
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
    use super::*;

    /// FR-33: the JSON form returns one row per registered tool with
    /// the annotation hint columns wired up. Matches the MCP
    /// `describe_tools` shape so consumers can switch surfaces
    /// without re-parsing.
    #[test]
    fn tools_json_lists_every_registered_tool_with_hint_columns() {
        let rows: Vec<ToolRow> = registered_tool_attrs()
            .into_iter()
            .map(|tool| {
                let ann = tool.annotations.as_ref();
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
                }
            })
            .collect();
        assert!(!rows.is_empty());
        for row in &rows {
            assert!(!row.name.is_empty(), "tool name must not be empty");
            assert!(
                row.title
                    .as_deref()
                    .map(|t| !t.is_empty())
                    .unwrap_or(false),
                "{}: title must be non-empty",
                row.name,
            );
        }
        // Spot-check a known tool to lock the rendering of the
        // hint columns once.
        let read_memory = rows
            .iter()
            .find(|r| r.name == "read_memory")
            .expect("read_memory present");
        assert_eq!(read_memory.read_only, Some(true));
        assert_eq!(read_memory.idempotent, Some(true));
        assert_eq!(read_memory.open_world, Some(false));
    }

    #[test]
    fn bool_cell_renders_three_states() {
        assert_eq!(bool_cell(Some(true)), "Y");
        assert_eq!(bool_cell(Some(false)), "n");
        assert_eq!(bool_cell(None), "-");
    }
}
