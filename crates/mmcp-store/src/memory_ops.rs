//! Transactional section-level editing for memory bodies (FR-026).
//!
//! The body parser in [`mmcp_core::memory::body`] slices a markdown
//! memory into an ordered sequence of addressable sections, each
//! with a stable dot-separated path id. This module supplies the
//! matching mutation surface: an ordered list of
//! [`MemoryEditOp`]s, applied sequentially to a body string.
//!
//! Semantics:
//!
//! - Ops run in order; each op re-parses the body so subsequent
//!   ops see the prior edits.
//! - The applier is transactional: the first error aborts the
//!   whole batch and the input body is returned unchanged at the
//!   caller's layer.
//! - Section operations address a whole section — its heading
//!   line plus every line down to the next peer or shallower
//!   heading. Nested sections move with their parent.
//! - Line-level ops (`InsertAtLine`, `ReplaceLines`, `DeleteLines`)
//!   exist as escape hatches for non-heading content (prose inside
//!   the preamble, code fences, plain lists). Section ops are the
//!   preferred surface because they survive rewrites of unrelated
//!   parts of the body.
//!
//! The wire shape matches the MCP tool's input schema directly so
//! a caller can ship the `ops` array verbatim from their tool
//! request into the applier.

use mmcp_core::memory::{BodyParseError, Section, parse_sections, render_sections};
use serde::{Deserialize, Serialize};

/// Single mutation of a memory body. See the module-level docs for
/// semantics; the variants are ordered section-first, line-last.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum MemoryEditOp {
    /// Create or replace a whole section (heading line + body).
    /// Matched by `path`. When absent, the new section is appended
    /// at the end of the body at the requested `level`.
    UpsertSection {
        path: String,
        level: u8,
        heading: String,
        body: String,
    },

    /// Remove a whole section. Nested subsections are removed
    /// together with their parent.
    DeleteSection { path: String },

    /// Insert a new section immediately before `anchor_path`. The
    /// anchor must exist.
    InsertSectionBefore {
        anchor_path: String,
        level: u8,
        heading: String,
        body: String,
    },

    /// Insert a new section immediately after `anchor_path`. The
    /// anchor must exist. When the anchor has children, the new
    /// section is inserted AFTER the whole anchor subtree so it
    /// lands as the anchor's next sibling.
    InsertSectionAfter {
        anchor_path: String,
        level: u8,
        heading: String,
        body: String,
    },

    /// Move `target_path` so it sits immediately before
    /// `anchor_path`. Both must exist and must not overlap.
    MoveSectionBefore {
        target_path: String,
        anchor_path: String,
    },

    /// Move `target_path` so it sits immediately after
    /// `anchor_path`'s subtree.
    MoveSectionAfter {
        target_path: String,
        anchor_path: String,
    },

    /// Replace the body of the section at `path` without touching
    /// the heading line or any nested subsections. The new `body`
    /// is the content between the heading and the next peer.
    ReplaceSectionBody { path: String, body: String },

    /// Insert `content` at the zero-based line index `line`.
    /// Lines at and after `line` shift down. `line` may equal the
    /// current line count to append at EOF.
    InsertAtLine { line: u32, content: String },

    /// Replace the half-open line range `[start, end)` with
    /// `content`. `end >= start`; `end <= current_line_count`.
    ReplaceLines {
        start: u32,
        end: u32,
        content: String,
    },

    /// Delete the half-open line range `[start, end)`. Same bounds
    /// as `ReplaceLines`.
    DeleteLines { start: u32, end: u32 },
}

/// Failure modes of [`apply_ops`]. Each variant carries enough
/// context for the MCP tool layer to map into a structured error
/// payload (`section_not_found`, `invalid_line_range`, etc.).
#[derive(Debug, thiserror::Error)]
pub enum MemoryEditError {
    /// A section target or anchor path did not resolve to any
    /// section in the current body snapshot.
    #[error("section `{path}` not found in body")]
    SectionNotFound { path: String },

    /// Move target and anchor resolve to the same section, or the
    /// target is an ancestor of the anchor. Both cases would loop.
    #[error("move op would loop: target `{target}` contains or equals anchor `{anchor}`")]
    MoveWouldLoop { target: String, anchor: String },

    /// A section op requested a heading level outside `1..=6`.
    /// Zero is reserved for the synthetic preamble.
    #[error("heading level {level} out of range (must be 1..=6)")]
    LevelOutOfRange { level: u8 },

    /// Line-range op arguments disagree (`end < start`) or a line
    /// index points past the end of the body.
    #[error("invalid line range: start={start}, end={end}, line_count={line_count}")]
    InvalidLineRange {
        start: u32,
        end: u32,
        line_count: u32,
    },

    /// `InsertAtLine` target is past the last valid insertion
    /// point. The valid range is `0..=line_count`.
    #[error("line {line} is past the end of the body ({line_count} lines)")]
    LinePastEof { line: u32, line_count: u32 },

    /// The parser itself rejected the body. Wrapped so the MCP
    /// tool can surface a structured error rather than an opaque
    /// failure.
    #[error(transparent)]
    Parse(#[from] BodyParseError),
}

/// Apply a sequence of ops to `body`, returning the rewritten
/// markdown. Ops are applied in order; each re-parses the body
/// so subsequent ops see the prior edits. The function is a pure
/// transformation — no I/O, no git — so tests drive it with plain
/// strings.
pub fn apply_ops(body: &str, ops: &[MemoryEditOp]) -> Result<String, MemoryEditError> {
    let mut current = body.to_string();
    for op in ops {
        current = apply_one(&current, op)?;
    }
    Ok(current)
}

fn apply_one(body: &str, op: &MemoryEditOp) -> Result<String, MemoryEditError> {
    match op {
        MemoryEditOp::UpsertSection {
            path,
            level,
            heading,
            body: section_body,
        } => upsert_section(body, path, *level, heading, section_body),
        MemoryEditOp::DeleteSection { path } => delete_section(body, path),
        MemoryEditOp::InsertSectionBefore {
            anchor_path,
            level,
            heading,
            body: section_body,
        } => insert_relative(body, anchor_path, *level, heading, section_body, true),
        MemoryEditOp::InsertSectionAfter {
            anchor_path,
            level,
            heading,
            body: section_body,
        } => insert_relative(body, anchor_path, *level, heading, section_body, false),
        MemoryEditOp::MoveSectionBefore {
            target_path,
            anchor_path,
        } => move_section(body, target_path, anchor_path, true),
        MemoryEditOp::MoveSectionAfter {
            target_path,
            anchor_path,
        } => move_section(body, target_path, anchor_path, false),
        MemoryEditOp::ReplaceSectionBody { path, body: new } => {
            replace_section_body(body, path, new)
        }
        MemoryEditOp::InsertAtLine { line, content } => insert_at_line(body, *line, content),
        MemoryEditOp::ReplaceLines {
            start,
            end,
            content,
        } => replace_lines(body, *start, *end, content),
        MemoryEditOp::DeleteLines { start, end } => replace_lines(body, *start, *end, ""),
    }
}

// ── Section ops ────────────────────────────────────────────────

fn upsert_section(
    body: &str,
    path: &str,
    level: u8,
    heading: &str,
    section_body: &str,
) -> Result<String, MemoryEditError> {
    require_level(level)?;
    let rendered = render_new_section(level, heading, section_body);
    let sections = parse_sections(body)?;
    match sections.iter().position(|s| s.path == path) {
        Some(idx) => {
            // Replace the full span of the existing section
            // (heading + body + nested). Compute the byte range
            // in the original body that the section occupies,
            // then splice.
            let range = section_byte_range(body, &sections, idx);
            let mut out = String::with_capacity(body.len() + rendered.len());
            out.push_str(&body[..range.0]);
            out.push_str(&rendered);
            out.push_str(&body[range.1..]);
            Ok(out)
        }
        None => {
            // Append at end, ensuring a blank line between the
            // prior content and the new section.
            let mut out = body.to_string();
            if !out.is_empty() && !out.ends_with("\n\n") {
                if !out.ends_with('\n') {
                    out.push('\n');
                }
                out.push('\n');
            }
            out.push_str(&rendered);
            Ok(out)
        }
    }
}

fn delete_section(body: &str, path: &str) -> Result<String, MemoryEditError> {
    let sections = parse_sections(body)?;
    let idx = sections
        .iter()
        .position(|s| s.path == path)
        .ok_or_else(|| MemoryEditError::SectionNotFound {
            path: path.to_string(),
        })?;
    // Preamble cannot be "deleted" — the section always exists.
    // Clear its content instead by collapsing its byte range.
    let (start, end) = section_byte_range(body, &sections, idx);
    let mut out = String::with_capacity(body.len() - (end - start));
    out.push_str(&body[..start]);
    out.push_str(&body[end..]);
    Ok(out)
}

fn insert_relative(
    body: &str,
    anchor_path: &str,
    level: u8,
    heading: &str,
    section_body: &str,
    before: bool,
) -> Result<String, MemoryEditError> {
    require_level(level)?;
    let sections = parse_sections(body)?;
    let idx = sections
        .iter()
        .position(|s| s.path == anchor_path)
        .ok_or_else(|| MemoryEditError::SectionNotFound {
            path: anchor_path.to_string(),
        })?;
    let rendered = render_new_section(level, heading, section_body);
    let (anchor_start, anchor_end) = section_byte_range(body, &sections, idx);
    let insertion_point = if before { anchor_start } else { anchor_end };

    let mut out = String::with_capacity(body.len() + rendered.len());
    out.push_str(&body[..insertion_point]);
    out.push_str(&rendered);
    out.push_str(&body[insertion_point..]);
    Ok(out)
}

fn move_section(
    body: &str,
    target_path: &str,
    anchor_path: &str,
    before: bool,
) -> Result<String, MemoryEditError> {
    if target_path == anchor_path {
        return Err(MemoryEditError::MoveWouldLoop {
            target: target_path.to_string(),
            anchor: anchor_path.to_string(),
        });
    }
    let sections = parse_sections(body)?;
    let target_idx = sections
        .iter()
        .position(|s| s.path == target_path)
        .ok_or_else(|| MemoryEditError::SectionNotFound {
            path: target_path.to_string(),
        })?;
    let anchor_idx = sections
        .iter()
        .position(|s| s.path == anchor_path)
        .ok_or_else(|| MemoryEditError::SectionNotFound {
            path: anchor_path.to_string(),
        })?;
    // Reject moves where the anchor sits inside the target's
    // subtree — otherwise the extracted block would loop.
    if anchor_idx > target_idx {
        let target_end_idx = subtree_end_idx(&sections, target_idx);
        if anchor_idx < target_end_idx {
            return Err(MemoryEditError::MoveWouldLoop {
                target: target_path.to_string(),
                anchor: anchor_path.to_string(),
            });
        }
    }

    let (target_start, target_end) = section_byte_range(body, &sections, target_idx);
    let extracted = body[target_start..target_end].to_string();

    // Remove the target from the body.
    let without_target = {
        let mut s = String::with_capacity(body.len() - (target_end - target_start));
        s.push_str(&body[..target_start]);
        s.push_str(&body[target_end..]);
        s
    };

    // Re-parse without the target so the anchor's byte range in
    // the new body is correct.
    let new_sections = parse_sections(&without_target)?;
    let anchor_idx = new_sections
        .iter()
        .position(|s| s.path == anchor_path)
        .ok_or_else(|| MemoryEditError::SectionNotFound {
            path: anchor_path.to_string(),
        })?;
    let (anchor_start, anchor_end) = section_byte_range(&without_target, &new_sections, anchor_idx);
    let insertion_point = if before { anchor_start } else { anchor_end };

    let mut out = String::with_capacity(without_target.len() + extracted.len());
    out.push_str(&without_target[..insertion_point]);
    out.push_str(&extracted);
    out.push_str(&without_target[insertion_point..]);
    Ok(out)
}

fn replace_section_body(
    body: &str,
    path: &str,
    new_section_body: &str,
) -> Result<String, MemoryEditError> {
    let sections = parse_sections(body)?;
    let idx = sections
        .iter()
        .position(|s| s.path == path)
        .ok_or_else(|| MemoryEditError::SectionNotFound {
            path: path.to_string(),
        })?;
    let section = &sections[idx];
    let lines: Vec<&str> = body.split_inclusive('\n').collect();

    // The section's "own body" is the lines from `line_start + 1`
    // (skip the heading) up to the first nested child's
    // `line_start` (or `line_end` if no children). The preamble
    // has no heading line, so its own body is everything within
    // its range.
    let (heading_lines, next_child_start) = if section.level == 0 {
        (0, section.line_end)
    } else {
        let mut next = section.line_end;
        if let Some(first_child) = sections
            .iter()
            .skip(idx + 1)
            .find(|c| c.level > section.level)
        {
            next = first_child.line_start;
        }
        (section.line_start + 1, next)
    };

    let start_byte = byte_offset_of_line(&lines, heading_lines);
    let end_byte = byte_offset_of_line(&lines, next_child_start);

    let mut out = String::with_capacity(body.len() + new_section_body.len());
    out.push_str(&body[..start_byte]);
    // Ensure a trailing newline after the body so the next
    // heading or EOF stays on its own line.
    let trimmed = new_section_body.trim_end_matches('\n');
    if !trimmed.is_empty() {
        out.push('\n');
        out.push_str(trimmed);
        out.push('\n');
    }
    out.push_str(&body[end_byte..]);
    Ok(out)
}

// ── Line ops ───────────────────────────────────────────────────

fn insert_at_line(body: &str, line: u32, content: &str) -> Result<String, MemoryEditError> {
    let lines: Vec<&str> = body.split_inclusive('\n').collect();
    let line_count = lines.len() as u32;
    if line > line_count {
        return Err(MemoryEditError::LinePastEof { line, line_count });
    }
    let mut out = String::with_capacity(body.len() + content.len() + 1);
    let insertion_byte = byte_offset_of_line(&lines, line as usize);
    out.push_str(&body[..insertion_byte]);
    out.push_str(content);
    if !content.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(&body[insertion_byte..]);
    Ok(out)
}

fn replace_lines(
    body: &str,
    start: u32,
    end: u32,
    content: &str,
) -> Result<String, MemoryEditError> {
    let lines: Vec<&str> = body.split_inclusive('\n').collect();
    let line_count = lines.len() as u32;
    if end < start || end > line_count {
        return Err(MemoryEditError::InvalidLineRange {
            start,
            end,
            line_count,
        });
    }
    let start_byte = byte_offset_of_line(&lines, start as usize);
    let end_byte = byte_offset_of_line(&lines, end as usize);
    let mut out = String::with_capacity(body.len() - (end_byte - start_byte) + content.len() + 1);
    out.push_str(&body[..start_byte]);
    if !content.is_empty() {
        out.push_str(content);
        if !content.ends_with('\n') {
            out.push('\n');
        }
    }
    out.push_str(&body[end_byte..]);
    Ok(out)
}

// ── Helpers ────────────────────────────────────────────────────

fn require_level(level: u8) -> Result<(), MemoryEditError> {
    if (1..=6).contains(&level) {
        Ok(())
    } else {
        Err(MemoryEditError::LevelOutOfRange { level })
    }
}

/// Render a fresh section at the given level with the given body.
/// Body is trimmed and framed with trailing newlines so the
/// splice always lands on clean boundaries.
fn render_new_section(level: u8, heading: &str, section_body: &str) -> String {
    let mut out = String::with_capacity(level as usize + heading.len() + section_body.len() + 8);
    for _ in 0..level {
        out.push('#');
    }
    out.push(' ');
    out.push_str(heading.trim());
    out.push('\n');
    let body = section_body.trim_matches('\n');
    if !body.is_empty() {
        out.push('\n');
        out.push_str(body);
        out.push('\n');
    }
    out.push('\n');
    out
}

/// Byte range `[start, end)` in the original body covering the
/// section at `sections[idx]` plus every nested subsection whose
/// level is deeper than the target.
fn section_byte_range(body: &str, sections: &[Section], idx: usize) -> (usize, usize) {
    let lines: Vec<&str> = body.split_inclusive('\n').collect();
    let section = &sections[idx];
    let end_idx = subtree_end_idx(sections, idx);
    let end_line = if end_idx < sections.len() {
        sections[end_idx].line_start
    } else {
        lines.len()
    };
    let start_byte = byte_offset_of_line(&lines, section.line_start);
    let end_byte = byte_offset_of_line(&lines, end_line);
    (start_byte, end_byte)
}

/// Index of the first sibling / ancestor section after the subtree
/// rooted at `sections[idx]`. Equal to `sections.len()` when the
/// subtree runs to EOF.
fn subtree_end_idx(sections: &[Section], idx: usize) -> usize {
    let root_level = sections[idx].level;
    for (offset, section) in sections.iter().enumerate().skip(idx + 1) {
        if section.level <= root_level {
            return offset;
        }
    }
    sections.len()
}

/// Convert a line index (zero-based) into a byte offset into the
/// original body. `line == line_count` maps to the end-of-body
/// byte (appending past the last line).
fn byte_offset_of_line(lines: &[&str], line: usize) -> usize {
    lines.iter().take(line).map(|l| l.len()).sum()
}

// Silence a dead-code warning on a helper we may reach for
// later — `render_sections` is available from mmcp-core, but
// `memory_ops` does not currently call it because every op
// mutates the raw body directly rather than going through the
// section renderer. Kept in the import list so future ops can
// adopt it without a re-plumb.
#[allow(dead_code)]
fn _unused_render_sections(body: &str, sections: &[Section]) -> String {
    render_sections(body, sections)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
preface paragraph

## Need

need body

## Resolution

resolution body

### Non-goals

non-goals body

## Done

done body
";

    #[test]
    fn upsert_existing_section_replaces_it() {
        let out = apply_ops(
            SAMPLE,
            &[MemoryEditOp::UpsertSection {
                path: "need".into(),
                level: 2,
                heading: "Need".into(),
                body: "fresh need content".into(),
            }],
        )
        .expect("apply");
        assert!(out.contains("## Need\n\nfresh need content\n"));
        assert!(!out.contains("need body"));
        // Other sections untouched.
        assert!(out.contains("resolution body"));
        assert!(out.contains("non-goals body"));
    }

    #[test]
    fn upsert_unknown_section_appends_to_eof() {
        let out = apply_ops(
            SAMPLE,
            &[MemoryEditOp::UpsertSection {
                path: "references".into(),
                level: 2,
                heading: "References".into(),
                body: "link one".into(),
            }],
        )
        .expect("apply");
        assert!(out.contains("## Done"));
        assert!(out.trim_end().ends_with("link one"));
    }

    #[test]
    fn delete_section_removes_heading_and_nested() {
        let out = apply_ops(
            SAMPLE,
            &[MemoryEditOp::DeleteSection {
                path: "resolution".into(),
            }],
        )
        .expect("apply");
        assert!(!out.contains("## Resolution"));
        assert!(!out.contains("resolution body"));
        assert!(!out.contains("### Non-goals"), "nested child must go too");
        // Preceding and following sections survive.
        assert!(out.contains("## Need"));
        assert!(out.contains("## Done"));
    }

    #[test]
    fn delete_missing_section_errors() {
        let err = apply_ops(
            SAMPLE,
            &[MemoryEditOp::DeleteSection {
                path: "nonexistent".into(),
            }],
        )
        .expect_err("must error");
        assert!(matches!(err, MemoryEditError::SectionNotFound { .. }));
    }

    #[test]
    fn insert_section_before_anchor() {
        let out = apply_ops(
            SAMPLE,
            &[MemoryEditOp::InsertSectionBefore {
                anchor_path: "need".into(),
                level: 2,
                heading: "Scope".into(),
                body: "scope body".into(),
            }],
        )
        .expect("apply");
        let scope_pos = out.find("## Scope").expect("scope landed");
        let need_pos = out.find("## Need").expect("need still present");
        assert!(scope_pos < need_pos, "scope must precede need");
    }

    #[test]
    fn insert_section_after_anchor_lands_past_subtree() {
        let out = apply_ops(
            SAMPLE,
            &[MemoryEditOp::InsertSectionAfter {
                anchor_path: "resolution".into(),
                level: 2,
                heading: "Tests".into(),
                body: "tests body".into(),
            }],
        )
        .expect("apply");
        let non_goals_pos = out.find("### Non-goals").expect("non-goals present");
        let tests_pos = out.find("## Tests").expect("tests landed");
        let done_pos = out.find("## Done").expect("done present");
        assert!(
            non_goals_pos < tests_pos,
            "tests must land AFTER the non-goals subsection"
        );
        assert!(tests_pos < done_pos, "tests must precede done");
    }

    #[test]
    fn move_section_before_anchor() {
        let out = apply_ops(
            SAMPLE,
            &[MemoryEditOp::MoveSectionBefore {
                target_path: "done".into(),
                anchor_path: "resolution".into(),
            }],
        )
        .expect("apply");
        let done_pos = out.find("## Done").expect("done present");
        let resolution_pos = out.find("## Resolution").expect("resolution present");
        assert!(
            done_pos < resolution_pos,
            "done must now precede resolution"
        );
    }

    #[test]
    fn move_section_after_anchor_subtree() {
        let out = apply_ops(
            SAMPLE,
            &[MemoryEditOp::MoveSectionAfter {
                target_path: "need".into(),
                anchor_path: "resolution".into(),
            }],
        )
        .expect("apply");
        let resolution_pos = out.find("## Resolution").expect("resolution present");
        let non_goals_pos = out.find("### Non-goals").expect("non-goals still nested");
        let need_pos = out.find("## Need").expect("need present");
        let done_pos = out.find("## Done").expect("done present");
        assert!(
            resolution_pos < non_goals_pos && non_goals_pos < need_pos,
            "need must land after the full resolution subtree"
        );
        assert!(need_pos < done_pos, "need must still precede done");
    }

    #[test]
    fn move_target_equals_anchor_errors() {
        let err = apply_ops(
            SAMPLE,
            &[MemoryEditOp::MoveSectionBefore {
                target_path: "need".into(),
                anchor_path: "need".into(),
            }],
        )
        .expect_err("must error");
        assert!(matches!(err, MemoryEditError::MoveWouldLoop { .. }));
    }

    #[test]
    fn replace_section_body_keeps_heading_and_children() {
        let out = apply_ops(
            SAMPLE,
            &[MemoryEditOp::ReplaceSectionBody {
                path: "resolution".into(),
                body: "rewritten resolution".into(),
            }],
        )
        .expect("apply");
        assert!(out.contains("## Resolution\n\nrewritten resolution\n"));
        assert!(!out.contains("resolution body"));
        // Nested non-goals subsection stays in place.
        assert!(out.contains("### Non-goals\n\nnon-goals body"));
    }

    #[test]
    fn insert_at_line_adds_content_at_index() {
        let body = "alpha\nbeta\ngamma\n";
        let out = apply_ops(
            body,
            &[MemoryEditOp::InsertAtLine {
                line: 1,
                content: "inserted".into(),
            }],
        )
        .expect("apply");
        assert_eq!(out, "alpha\ninserted\nbeta\ngamma\n");
    }

    #[test]
    fn insert_at_line_past_eof_errors() {
        let body = "one\ntwo\n";
        let err = apply_ops(
            body,
            &[MemoryEditOp::InsertAtLine {
                line: 99,
                content: "x".into(),
            }],
        )
        .expect_err("must error");
        assert!(matches!(err, MemoryEditError::LinePastEof { .. }));
    }

    #[test]
    fn replace_lines_swaps_range() {
        let body = "alpha\nbeta\ngamma\ndelta\n";
        let out = apply_ops(
            body,
            &[MemoryEditOp::ReplaceLines {
                start: 1,
                end: 3,
                content: "new middle".into(),
            }],
        )
        .expect("apply");
        assert_eq!(out, "alpha\nnew middle\ndelta\n");
    }

    #[test]
    fn delete_lines_collapses_range() {
        let body = "alpha\nbeta\ngamma\n";
        let out =
            apply_ops(body, &[MemoryEditOp::DeleteLines { start: 1, end: 2 }]).expect("apply");
        assert_eq!(out, "alpha\ngamma\n");
    }

    #[test]
    fn invalid_line_range_errors() {
        let body = "a\nb\n";
        let err = apply_ops(
            body,
            &[MemoryEditOp::ReplaceLines {
                start: 3,
                end: 1,
                content: "x".into(),
            }],
        )
        .expect_err("must error");
        assert!(matches!(err, MemoryEditError::InvalidLineRange { .. }));
    }

    #[test]
    fn level_out_of_range_rejected() {
        let err = apply_ops(
            SAMPLE,
            &[MemoryEditOp::UpsertSection {
                path: "invalid".into(),
                level: 7,
                heading: "Too Deep".into(),
                body: "".into(),
            }],
        )
        .expect_err("must error");
        assert!(matches!(err, MemoryEditError::LevelOutOfRange { .. }));
    }

    #[test]
    fn composition_insert_then_move() {
        // Compose: insert a new section, then move an existing one
        // before it. Exercises the re-parse-per-op invariant.
        let out = apply_ops(
            SAMPLE,
            &[
                MemoryEditOp::InsertSectionAfter {
                    anchor_path: "resolution".into(),
                    level: 2,
                    heading: "Tests".into(),
                    body: "tests body".into(),
                },
                MemoryEditOp::MoveSectionBefore {
                    target_path: "tests".into(),
                    anchor_path: "need".into(),
                },
            ],
        )
        .expect("apply");
        let tests_pos = out.find("## Tests").expect("tests present");
        let need_pos = out.find("## Need").expect("need present");
        assert!(tests_pos < need_pos, "tests must now precede need");
    }

    #[test]
    fn composition_upsert_then_delete() {
        let out = apply_ops(
            SAMPLE,
            &[
                MemoryEditOp::UpsertSection {
                    path: "references".into(),
                    level: 2,
                    heading: "References".into(),
                    body: "link one".into(),
                },
                MemoryEditOp::DeleteSection {
                    path: "references".into(),
                },
            ],
        )
        .expect("apply");
        assert!(!out.contains("## References"));
    }

    #[test]
    fn composition_replace_then_insert() {
        let out = apply_ops(
            SAMPLE,
            &[
                MemoryEditOp::ReplaceSectionBody {
                    path: "need".into(),
                    body: "new need".into(),
                },
                MemoryEditOp::InsertSectionAfter {
                    anchor_path: "need".into(),
                    level: 3,
                    heading: "Rationale".into(),
                    body: "why".into(),
                },
            ],
        )
        .expect("apply");
        assert!(out.contains("## Need\n\nnew need"));
        assert!(out.contains("### Rationale\n\nwhy"));
    }
}
