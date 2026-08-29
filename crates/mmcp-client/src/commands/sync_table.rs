//! Per-group outcome table for `mmcp sync` / `pull` / `push` / `fetch`.
//!
//! Renders through `comfy_table`.
//! A single `plain` decision picks the preset (Unicode box vs ASCII) and whether cells get color.
//! `NO_COLOR` (<https://no-color.org>, any value counts) or a non-terminal stdout both force plain output.
//! Plain output carries no ANSI escape and no box-drawing glyph.
//!
//! [`render_with`] takes `plain` as a parameter instead of reading the environment or stdout itself.
//! This keeps both branches testable without touching global process state.
//! This workspace forbids `unsafe` code, and `std::env::set_var` requires it.
//! [`render_sync_table`] is the thin wrapper real callers use.

use std::io::IsTerminal;

use comfy_table::{Cell, Color, ContentArrangement, Table, presets};

/// One outcome row: a group, optionally attributed to a remote.
#[derive(Debug, Clone)]
pub struct SyncTableRow {
    pub group: String,
    pub remote: Option<String>,
    pub action: SyncRowAction,
    pub detail: Option<String>,
}

impl SyncTableRow {
    #[must_use]
    pub fn new(group: impl Into<String>, action: SyncRowAction) -> Self {
        Self {
            group: group.into(),
            remote: None,
            action,
            detail: None,
        }
    }

    #[must_use]
    pub fn with_remote(mut self, remote: impl Into<String>) -> Self {
        self.remote = Some(remote.into());
        self
    }

    #[must_use]
    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }
}

/// What happened to one group during a sync verb.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncRowAction {
    New,
    Updated,
    Pushed,
    /// Content-plane transfer skipped though the control plane succeeded.
    /// See `PushedGroup::content_transferred` and `FetchedGroup::ref_updated`.
    Skipped,
    Failed,
    /// A remote's manifest poll itself failed before any group-level candidate could be built for it.
    Unreachable,
}

impl SyncRowAction {
    fn label(self) -> &'static str {
        match self {
            Self::New => "new",
            Self::Updated => "updated",
            Self::Pushed => "pushed",
            Self::Skipped => "skipped",
            Self::Failed => "failed",
            Self::Unreachable => "unreachable",
        }
    }
}

/// True when the operator asked for plain, colorless, ASCII-only output: `NO_COLOR` set, or stdout is not a terminal.
#[must_use]
fn plain_output_requested() -> bool {
    std::env::var_os("NO_COLOR").is_some() || !std::io::stdout().is_terminal()
}

/// Render `rows` as a table, honoring the live terminal and `NO_COLOR` state.
/// Returns `None` for an empty slice: a quiet outcome prints nothing, per clig.dev's advice against needless output.
#[must_use]
pub fn render_sync_table(rows: &[SyncTableRow]) -> Option<String> {
    render_with(rows, plain_output_requested())
}

/// Pure rendering core behind [`render_sync_table`].
/// See the module doc comment for why `plain` is a parameter, not an internal environment read.
fn render_with(rows: &[SyncTableRow], plain: bool) -> Option<String> {
    if rows.is_empty() {
        return None;
    }
    let mut table = Table::new();
    if plain {
        table.force_no_tty();
    } else {
        table.enforce_styling();
    }
    let style = if plain {
        presets::ASCII_FULL
    } else {
        presets::UTF8_FULL
    };
    table
        .load_style(style)
        .set_content_arrangement(ContentArrangement::Dynamic)
        .set_header(vec!["group", "remote", "action", "detail"]);
    for row in rows {
        let mut action_cell = Cell::new(row.action.label());
        if !plain && row.action == SyncRowAction::Failed {
            action_cell = action_cell.fg(Color::Red);
        }
        table.add_row(vec![
            Cell::new(&row.group),
            Cell::new(row.remote.as_deref().unwrap_or("-")),
            action_cell,
            Cell::new(row.detail.as_deref().unwrap_or("")),
        ]);
    }
    Some(table.to_string())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn render_with_empty_rows_prints_nothing() {
        assert_eq!(render_with(&[], false), None);
        assert_eq!(render_with(&[], true), None);
    }

    #[test]
    fn render_with_one_row_lists_its_fields() {
        let rows = vec![SyncTableRow::new("alpha", SyncRowAction::Updated).with_remote("primary")];
        let out = render_with(&rows, true).expect("non-empty rows render a table");
        assert!(out.contains("alpha"));
        assert!(out.contains("primary"));
        assert!(out.contains("updated"));
    }

    #[test]
    fn render_with_multiple_rows_lists_each_group() {
        let rows = vec![
            SyncTableRow::new("alpha", SyncRowAction::New),
            SyncTableRow::new("beta", SyncRowAction::Unreachable),
        ];
        let out = render_with(&rows, true).expect("rows render");
        assert!(out.contains("alpha"));
        assert!(out.contains("new"));
        assert!(out.contains("beta"));
        assert!(out.contains("unreachable"));
    }

    #[test]
    fn render_with_failed_row_shows_the_error_detail() {
        let rows =
            vec![SyncTableRow::new("gamma", SyncRowAction::Failed).with_detail("connection reset")];
        let out = render_with(&rows, true).expect("rows render");
        assert!(out.contains("failed"));
        assert!(out.contains("connection reset"));
    }

    #[test]
    fn plain_mode_emits_no_ansi_escape_or_unicode_box_drawing() {
        let rows = vec![SyncTableRow::new("gamma", SyncRowAction::Failed).with_detail("boom")];
        let out = render_with(&rows, true).expect("rows render");
        assert!(
            !out.contains('\u{1b}'),
            "plain output must carry no ANSI escape: {out}"
        );
        assert!(
            !out.contains('\u{2500}'),
            "plain output must carry no Unicode box-drawing: {out}"
        );
    }

    #[test]
    fn fancy_mode_colors_a_failed_row_and_uses_unicode_borders() {
        let rows = vec![SyncTableRow::new("gamma", SyncRowAction::Failed).with_detail("boom")];
        let out = render_with(&rows, false).expect("rows render");
        assert!(
            out.contains('\u{1b}'),
            "fancy output must color the failed row: {out}"
        );
        assert!(
            out.contains('\u{2500}'),
            "fancy output must use Unicode box-drawing: {out}"
        );
    }
}
