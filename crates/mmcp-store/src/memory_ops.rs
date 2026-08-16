//! Transactional section-level editing for memory bodies.
//!
//! The body parser in [`mmcp_core::memory::body`] slices a markdown memory into addressable sections,
//! each with a stable dot-separated path id.
//! This module supplies the matching mutation surface:
//! an ordered list of [`MemoryEditOp`]s, applied sequentially to a body string.
//!
//! Semantics:
//!
//! - Ops run in order; each op re-parses the body so subsequent ops see the prior edits.
//! - The applier is transactional:
//!   the first error aborts the whole batch,
//!   and the input body is returned unchanged at the caller's layer.
//! - Section operations address a whole section:
//!   its heading line plus every line down to the next peer or shallower heading.
//!   Nested sections move with their parent.
//! - Line-level ops (`InsertAtLine`, `ReplaceLines`, `DeleteLines`) exist as escape hatches
//!   for non-heading content, prose inside the preamble, code fences, plain lists.
//!   Section ops are the preferred surface, because they survive rewrites of unrelated parts of the body.
//!
//! Every op resolves to a byte range plus a replacement fragment,
//! and hands both to [`splice`], which owns the seam rules.
//! No op patches newlines itself:
//! a local patch is what lets an insertion merge into the preceding line or sit flush against the next heading.
//!
//! The wire shape matches the MCP tool's input schema directly,
//! so a caller can ship the `ops` array verbatim from their tool request into the applier.

use std::ops::Range;

use mmcp_core::memory::{
    BodyParseError, Section, SpliceError, line_terminator, parse_sections, splice,
};
use serde::{Deserialize, Serialize};

/// Single mutation of a memory body.
/// See the module-level docs for semantics;
/// the variants are ordered section-first, line-last.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum MemoryEditOp {
    /// Create or replace a whole section (heading line + body).
    /// Matched by `path`.
    /// When absent, the new section is appended at the end of the body at the requested `level`.
    UpsertSection {
        path: String,
        level: u8,
        heading: String,
        body: String,
    },

    /// Remove a whole section.
    /// Nested subsections are removed together with their parent.
    DeleteSection { path: String },

    /// Insert a new section immediately before `anchor_path`.
    /// The anchor must exist.
    InsertSectionBefore {
        anchor_path: String,
        level: u8,
        heading: String,
        body: String,
    },

    /// Insert a new section immediately after `anchor_path`.
    /// The anchor must exist.
    /// When the anchor has children, the new section is inserted AFTER the whole anchor subtree
    /// so it lands as the anchor's next sibling.
    InsertSectionAfter {
        anchor_path: String,
        level: u8,
        heading: String,
        body: String,
    },

    /// Move `target_path` so it sits immediately before `anchor_path`.
    /// Both must exist and must not overlap.
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

    /// Replace the body of the section at `path` without touching the heading line or any nested subsections.
    /// The new `body` is the content between the heading and the next peer.
    ReplaceSectionBody { path: String, body: String },

    /// Insert `content` at the zero-based line index `line`.
    /// Lines at and after `line` shift down.
    /// `line` may equal the current line count to append at EOF.
    InsertAtLine { line: u32, content: String },

    /// Replace the half-open line range `[start, end)` with `content`.
    /// `end >= start`; `end <= current_line_count`.
    ReplaceLines {
        start: u32,
        end: u32,
        content: String,
    },

    /// Delete the half-open line range `[start, end)`.
    /// Same bounds as `ReplaceLines`.
    DeleteLines { start: u32, end: u32 },
}

/// Heading level reserved for the synthetic preamble section.
const PREAMBLE_LEVEL: u8 = 0;

/// Failure modes of [`apply_ops`].
/// Each variant carries enough context for the MCP tool layer to map into a structured error payload
/// (`section_not_found`, `invalid_line_range`, etc.).
#[derive(Debug, thiserror::Error)]
pub enum MemoryEditError {
    /// A section target or anchor path did not resolve to any
    /// section in the current body snapshot.
    #[error("section `{path}` not found in body")]
    SectionNotFound { path: String },

    /// Move target and anchor resolve to the same section, or the target is an ancestor of the anchor.
    /// Both cases would loop.
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

    /// `InsertAtLine` target is past the last valid insertion point.
    /// The valid range is `0..=line_count`.
    #[error("line {line} is past the end of the body ({line_count} lines)")]
    LinePastEof { line: u32, line_count: u32 },

    /// The parser itself rejected the body.
    /// Wrapped so the MCP tool can surface a structured error rather than an opaque failure.
    #[error(transparent)]
    Parse(#[from] BodyParseError),

    /// A resolved byte range did not index the body.
    /// Signals a bug in the range computation rather than bad caller input,
    /// since every range is derived from a section span or a line index.
    #[error(transparent)]
    Splice(#[from] SpliceError),

    /// `UpsertSection` was aimed at the synthetic preamble.
    /// Upsert renders a heading, and the preamble is by definition the span before the first heading,
    /// so the two cannot both hold.
    /// `ReplaceSectionBody` is the op that rewrites preamble prose.
    #[error("the preamble holds no heading; use replace_section_body to rewrite it")]
    PreambleNotUpsertable,

    /// Appending the section produced a different path than the one requested,
    /// because the new heading nests under the document's last section instead of becoming its sibling.
    /// Left unreported, the next upsert would miss the same path and append again, without bound.
    #[error(
        "upsert of `{requested}` at this level appends as `{produced}`; target that path instead"
    )]
    UpsertPathUnreachable { requested: String, produced: String },

    /// The section was written but does not contain everything that was rendered into it.
    /// A heading in the supplied body at the section's own level or shallower closes the section instead of nesting inside it,
    /// and a body leaving a code fence open stops the heading from parsing as one at all.
    /// Either way the path no longer addresses the whole section, so the next upsert appends a second copy.
    #[error(
        "upsert of `{requested}` wrote a section that does not contain its own body; the body must nest below the section's level and must not leave a construct open"
    )]
    UpsertSectionNotSelfContained { requested: String },
}

/// Apply a sequence of ops to `body`, returning the rewritten markdown.
/// Ops are applied in order; each re-parses the body so subsequent ops see the prior edits.
/// The function is a pure transformation, no I/O, no git,
/// so tests drive it with plain strings.
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
    // Upsert always renders a heading, so aiming it at the preamble asks for a heading
    // in the span defined as preceding the first heading.
    // Left unguarded, this would degenerate into a plain insert at offset zero,
    // prepending a fresh copy on every call.
    if path == Section::PREAMBLE_PATH {
        return Err(MemoryEditError::PreambleNotUpsertable);
    }
    let rendered = render_new_section(level, heading, section_body, line_terminator(body));
    let sections = parse_sections(body)?;

    // A known path replaces the section's full span (heading + body + nested),
    // and the replacement lands at the same index.
    // An unknown one appends at end of body, so it lands one past the last existing section.
    let (spliced, written_idx) = match sections.iter().position(|s| s.path == path) {
        Some(idx) => (
            splice(body, section_byte_range(body, &sections, idx), &rendered)?,
            idx,
        ),
        None => (
            splice(body, body.len()..body.len(), &rendered)?,
            sections.len(),
        ),
    };

    // The written section must come back addressable at the requested path
    // AND must still contain everything that was rendered into it.
    // Either failure leaves a section the caller cannot target again,
    // so the next upsert appends another copy, without bound.
    let sections_after = parse_sections(&spliced)?;
    let Some(written) = sections_after.get(written_idx) else {
        // The rendered heading did not survive as a heading at all.
        // This happens when the append lands inside a construct the body left open,
        // such as an unterminated code fence.
        return Err(MemoryEditError::UpsertSectionNotSelfContained {
            requested: path.to_string(),
        });
    };

    // The path comes from the heading trail.
    // Nesting one level deeper than the neighbouring heading yields a different path than the one requested.
    if written.path != path {
        return Err(MemoryEditError::UpsertPathUnreachable {
            requested: path.to_string(),
            produced: written.path.clone(),
        });
    }

    // Conversely,
    // a heading in the body at the section's own level or shallower ends the section instead of nesting inside it,
    // so part of what was rendered lands outside the span the path addresses.
    // The span must cover every rendered line.
    let written_start = written.line_start;
    let subtree_end = subtree_end_idx(&sections_after, written_idx);
    let total_lines = spliced.split_inclusive('\n').count();
    let span_end = sections_after
        .get(subtree_end)
        .map_or(total_lines, |section| section.line_start);
    let rendered_lines = rendered.matches('\n').count() + 1;
    if span_end.saturating_sub(written_start) < rendered_lines {
        return Err(MemoryEditError::UpsertSectionNotSelfContained {
            requested: path.to_string(),
        });
    }
    Ok(spliced)
}

fn delete_section(body: &str, path: &str) -> Result<String, MemoryEditError> {
    let sections = parse_sections(body)?;
    let idx = sections
        .iter()
        .position(|s| s.path == path)
        .ok_or_else(|| MemoryEditError::SectionNotFound {
            path: path.to_string(),
        })?;
    // Preamble cannot be "deleted" - the section always exists.
    // Clear its content instead by collapsing its byte range.
    Ok(splice(body, section_byte_range(body, &sections, idx), "")?)
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
    let rendered = render_new_section(level, heading, section_body, line_terminator(body));
    let anchor = section_byte_range(body, &sections, idx);
    let point = if before { anchor.start } else { anchor.end };
    Ok(splice(body, point..point, &rendered)?)
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
    // Reject moves where the anchor sits inside the target's subtree:
    // otherwise the extracted block would loop.
    if anchor_idx > target_idx {
        let target_end_idx = subtree_end_idx(&sections, target_idx);
        if anchor_idx < target_end_idx {
            return Err(MemoryEditError::MoveWouldLoop {
                target: target_path.to_string(),
                anchor: anchor_path.to_string(),
            });
        }
    }

    let target = section_byte_range(body, &sections, target_idx);
    // Taken verbatim, blank lines included.
    // Those blanks are the author's spacing, and the destination seam only adds a separator
    // when one is missing, so carrying them across keeps a move and its inverse an identity.
    let extracted = body[target.clone()].to_string();
    let removal_reached_eof = target.end == body.len();
    let mut without_target = splice(body, target, "")?;
    // Lifting the final section leaves behind the blank line that separated it.
    // A deletion keeps that line, because the caller asked to remove a span and nothing else,
    // but a relocation must drop it, or moving a section away and back never restores the original body.
    if removal_reached_eof {
        drop_one_trailing_blank_line(&mut without_target);
    }

    // Re-parse without the target so the anchor's byte range in the new body is correct.
    let new_sections = parse_sections(&without_target)?;
    let anchor_idx = new_sections
        .iter()
        .position(|s| s.path == anchor_path)
        .ok_or_else(|| MemoryEditError::SectionNotFound {
            path: anchor_path.to_string(),
        })?;
    let anchor = section_byte_range(&without_target, &new_sections, anchor_idx);
    let point = if before { anchor.start } else { anchor.end };
    Ok(splice(&without_target, point..point, &extracted)?)
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

    // The section's own body runs from the end of its heading construct to its first real child,
    // or to the end of its subtree when it has none.
    // The heading's line span comes from the parser: assuming one line eats a setext underline
    // and demotes the heading to a paragraph.
    // `line_end` is not usable here because it stops at the next heading of any kind,
    // including one quoted inside this very section.
    let subtree_end = subtree_end_idx(&sections, idx);
    let subtree_end_line = sections
        .get(subtree_end)
        .map_or(lines.len(), |next| next.line_start);
    let own_body_end = sections
        .iter()
        .skip(idx + 1)
        .take(subtree_end.saturating_sub(idx + 1))
        .find(|child| child.container_depth == 0 && child.level > section.level)
        .map_or(subtree_end_line, |child| child.line_start);

    let start_byte = byte_offset_of_line(&lines, section.heading_line_end);
    let end_byte = byte_offset_of_line(&lines, own_body_end);
    Ok(splice(
        body,
        start_byte..end_byte,
        new_section_body.trim_matches('\n'),
    )?)
}

// ── Line ops ───────────────────────────────────────────────────

fn insert_at_line(body: &str, line: u32, content: &str) -> Result<String, MemoryEditError> {
    let lines: Vec<&str> = body.split_inclusive('\n').collect();
    let line_count = lines.len() as u32;
    if line > line_count {
        return Err(MemoryEditError::LinePastEof { line, line_count });
    }
    let point = byte_offset_of_line(&lines, line as usize);
    Ok(splice(body, point..point, content)?)
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
    Ok(splice(body, start_byte..end_byte, content)?)
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
///
/// The fragment carries no leading or trailing padding: the splice it
/// feeds owns every seam, so padding here would only be collapsed
/// again. `terminator` applies to the blank line this function emits
/// between heading and body; `section_body` is the author's text and
/// goes in verbatim.
fn render_new_section(level: u8, heading: &str, section_body: &str, terminator: &str) -> String {
    let mut out = String::with_capacity(level as usize + heading.len() + section_body.len() + 4);
    for _ in 0..level {
        out.push('#');
    }
    out.push(' ');
    out.push_str(heading.trim());
    let body = section_body.trim_matches('\n');
    if !body.is_empty() {
        out.push_str(terminator);
        out.push_str(terminator);
        out.push_str(body);
    }
    out
}

/// Byte range `[start, end)` in the original body covering the
/// section at `sections[idx]` plus every nested subsection whose
/// level is deeper than the target.
fn section_byte_range(body: &str, sections: &[Section], idx: usize) -> Range<usize> {
    let lines: Vec<&str> = body.split_inclusive('\n').collect();
    let section = &sections[idx];
    let end_idx = subtree_end_idx(sections, idx);
    let end_line = if end_idx < sections.len() {
        sections[end_idx].line_start
    } else {
        lines.len()
    };
    byte_offset_of_line(&lines, section.line_start)..byte_offset_of_line(&lines, end_line)
}

/// Index of the first sibling / ancestor section after the subtree rooted at `sections[idx]`.
/// Equal to `sections.len()` when the subtree runs to EOF.
///
/// The synthetic preamble sits at level zero and owns no subtree:
/// it is the span before the first heading, not an ancestor of it.
/// Every real heading is level one or deeper,
/// so a level comparison alone would hand the preamble the whole document
/// and turn a preamble edit into a full-body wipe.
fn subtree_end_idx(sections: &[Section], idx: usize) -> usize {
    let root = &sections[idx];
    // Neither the preamble nor a heading nested in a container roots a subtree of the document outline:
    // the preamble is the span before the first heading,
    // and a contained heading owns only its own lines inside its quote or list item.
    if root.level == PREAMBLE_LEVEL || root.container_depth > 0 {
        return (idx + 1).min(sections.len());
    }
    for (offset, section) in sections.iter().enumerate().skip(idx + 1) {
        // A heading inside a block quote or a list item belongs to that container, not to the outline,
        // so it never closes the section it happens to sit in.
        // Treating it as a peer cuts a section's span short,
        // which truncates a delete or a move without a word.
        if section.container_depth > 0 {
            continue;
        }
        if section.level <= root.level {
            return offset;
        }
    }
    sections.len()
}

/// Remove one trailing blank line, leaving every other byte as it stands.
/// Trimming the whole run instead would delete blank lines the author wrote,
/// and rewriting the surviving terminator would downgrade a CRLF line ending nothing asked to touch.
fn drop_one_trailing_blank_line(text: &mut String) {
    let Some(head) = text.strip_suffix('\n') else {
        return;
    };
    let head = head.strip_suffix('\r').unwrap_or(head);
    // What remains still closes with a terminator only when the text ended on an empty line.
    if !(head.is_empty() || head.ends_with('\n')) {
        return;
    }
    let terminator_len = if text.ends_with("\r\n") { 2 } else { 1 };
    text.truncate(text.len() - terminator_len);
}

/// Convert a line index (zero-based) into a byte offset into the original body.
/// `line == line_count` maps to the end-of-body byte (appending past the last line).
fn byte_offset_of_line(lines: &[&str], line: usize) -> usize {
    lines.iter().take(line).map(|l| l.len()).sum()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
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

    /// Stored bodies carry no trailing newline, so every append at
    /// end of body exercises the seam that used to merge the
    /// insertion into the last line.
    #[test]
    fn insert_at_line_at_eof_does_not_merge_into_the_last_line() {
        let body = "alpha\nbeta";
        let out = apply_ops(
            body,
            &[MemoryEditOp::InsertAtLine {
                line: 2,
                content: "gamma".into(),
            }],
        )
        .expect("apply");
        assert_eq!(out, "alpha\nbeta\ngamma\n");
    }

    #[test]
    fn replace_lines_at_eof_does_not_merge_into_the_last_line() {
        let body = "alpha\nbeta";
        let out = apply_ops(
            body,
            &[MemoryEditOp::ReplaceLines {
                start: 2,
                end: 2,
                content: "gamma".into(),
            }],
        )
        .expect("apply");
        assert_eq!(out, "alpha\nbeta\ngamma\n");
    }

    #[test]
    fn insert_section_after_last_section_does_not_merge_into_the_last_line() {
        let body = "## Need\n\nneed body";
        let out = apply_ops(
            body,
            &[MemoryEditOp::InsertSectionAfter {
                anchor_path: "need".into(),
                level: 2,
                heading: "Done".into(),
                body: "done body".into(),
            }],
        )
        .expect("apply");
        assert_eq!(out, "## Need\n\nneed body\n\n## Done\n\ndone body\n");
    }

    #[test]
    fn replace_section_body_keeps_a_blank_line_before_the_next_heading() {
        let out = apply_ops(
            SAMPLE,
            &[MemoryEditOp::ReplaceSectionBody {
                path: "resolution".into(),
                body: "rewritten resolution".into(),
            }],
        )
        .expect("apply");
        assert!(
            out.contains("rewritten resolution\n\n### Non-goals"),
            "next heading must keep its blank line; got:\n{out}",
        );
    }

    #[test]
    fn replace_preamble_body_does_not_glue_the_first_heading() {
        let body = "## Need\n\nneed body\n";
        let out = apply_ops(
            body,
            &[MemoryEditOp::ReplaceSectionBody {
                path: "preamble".into(),
                body: "intro line".into(),
            }],
        )
        .expect("apply");
        assert_eq!(out, "intro line\n\n## Need\n\nneed body\n");
    }

    #[test]
    fn insert_at_line_before_a_heading_keeps_a_blank_line() {
        let body = "intro\n\n## Need\n\nneed body\n";
        let out = apply_ops(
            body,
            &[MemoryEditOp::InsertAtLine {
                line: 2,
                content: "added note".into(),
            }],
        )
        .expect("apply");
        assert_eq!(out, "intro\n\nadded note\n\n## Need\n\nneed body\n");
    }

    /// The preamble is the span before the first heading, not the document root.
    /// Treating it as a root made every preamble edit swallow the entire body.
    #[test]
    fn delete_preamble_keeps_every_heading_section() {
        let body = "intro\n\n## Need\n\nneed body\n";
        let out = apply_ops(
            body,
            &[MemoryEditOp::DeleteSection {
                path: "preamble".into(),
            }],
        )
        .expect("apply");
        assert_eq!(out, "## Need\n\nneed body\n");
    }

    /// Upserting a heading onto the preamble is a category error.
    /// Accepting it degenerated into an unconditional insert at
    /// offset zero, so repeating the op prepended a new copy forever.
    #[test]
    fn upsert_over_the_preamble_is_rejected() {
        let err = apply_ops(
            "intro\n\n## Need\n\nneed body\n",
            &[MemoryEditOp::UpsertSection {
                path: "preamble".into(),
                level: 2,
                heading: "Scope".into(),
                body: "scope body".into(),
            }],
        )
        .expect_err("must error");
        assert!(matches!(err, MemoryEditError::PreambleNotUpsertable));
    }

    #[test]
    fn replace_section_body_preserves_a_setext_underline() {
        let out = apply_ops(
            "Title\n=====\n\nold body\n",
            &[MemoryEditOp::ReplaceSectionBody {
                path: "title".into(),
                body: "new body".into(),
            }],
        )
        .expect("apply");
        assert_eq!(out, "Title\n=====\n\nnew body\n");
    }

    /// A blank line before a setext heading's text line is required:
    /// without it the preceding content is absorbed into the heading
    /// and the section's addressable path disappears.
    #[test]
    fn insert_before_a_setext_section_keeps_it_addressable() {
        let out = apply_ops(
            "First\n=====\n\na\n\nSecond\n======\n\nb\n",
            &[MemoryEditOp::InsertSectionBefore {
                anchor_path: "second".into(),
                level: 2,
                heading: "Mid".into(),
                body: "m".into(),
            }],
        )
        .expect("apply");
        assert_eq!(
            out,
            "First\n=====\n\na\n\n## Mid\n\nm\n\nSecond\n======\n\nb\n"
        );
    }

    #[test]
    fn indented_atx_heading_still_gets_its_blank_line() {
        let out = apply_ops(
            "intro\n   ## Indented\n\nbody\n",
            &[MemoryEditOp::InsertAtLine {
                line: 1,
                content: "note".into(),
            }],
        )
        .expect("apply");
        assert_eq!(out, "intro\nnote\n\n   ## Indented\n\nbody\n");
    }

    #[test]
    fn heading_inside_a_list_item_keeps_its_container_intact() {
        let out = apply_ops(
            "- item\n  ## InList\n\nafter\n",
            &[MemoryEditOp::InsertAtLine {
                line: 1,
                content: "  note".into(),
            }],
        )
        .expect("apply");
        assert_eq!(out, "- item\n  note\n  ## InList\n\nafter\n");
    }

    /// An op addressing line zero must not rewrite the tail.
    /// Trailing blank lines are content, not padding to be normalized away.
    #[test]
    fn an_op_far_from_the_tail_leaves_the_tail_alone() {
        let out = apply_ops(
            "## A\n\na\n\n\n\n## B\n\nb\n\n\n",
            &[MemoryEditOp::InsertAtLine {
                line: 0,
                content: "X".into(),
            }],
        )
        .expect("apply");
        assert_eq!(out, "X\n\n## A\n\na\n\n\n\n## B\n\nb\n\n\n");
    }

    /// A deletion removes the requested span and nothing more.
    /// Blank lines around it are the author's content, including the ones left exposed at end of body.
    #[test]
    fn deleting_the_last_section_leaves_neighbouring_blank_lines_alone() {
        let out = apply_ops(
            "intro\n\n\n\n## B\n\nb\n",
            &[MemoryEditOp::DeleteSection { path: "b".into() }],
        )
        .expect("apply");
        assert_eq!(out, "intro\n\n\n\n");
    }

    #[test]
    fn deleting_one_trailing_blank_line_removes_exactly_one() {
        let out = apply_ops(
            "a\n\n\n\n",
            &[MemoryEditOp::DeleteLines { start: 3, end: 4 }],
        )
        .expect("apply");
        assert_eq!(out, "a\n\n\n");
    }

    #[test]
    fn deleting_inside_an_unclosed_fence_keeps_the_rest_of_the_fence() {
        let out = apply_ops(
            "## A\n\n```\nx\n\n\n",
            &[MemoryEditOp::DeleteLines { start: 5, end: 6 }],
        )
        .expect("apply");
        assert_eq!(out, "## A\n\n```\nx\n\n");
    }

    #[test]
    fn a_surviving_crlf_terminator_is_not_downgraded() {
        let out = apply_ops(
            "a\nb\nc\nd\r\n\r\n## Z\n\nz\n",
            &[MemoryEditOp::DeleteSection { path: "z".into() }],
        )
        .expect("apply");
        assert_eq!(out, "a\nb\nc\nd\r\n\r\n");
    }

    /// Appending a level deeper than the last heading nests the new section, so its path is not the one requested.
    /// Writing it anyway made every repeat append another copy.
    #[test]
    fn upsert_that_would_nest_under_the_last_section_is_rejected() {
        let err = apply_ops(
            "# A\n\na\n\n# B\n\nb\n",
            &[MemoryEditOp::UpsertSection {
                path: "zz".into(),
                level: 2,
                heading: "ZZ".into(),
                body: "z".into(),
            }],
        )
        .expect_err("must error");
        match err {
            MemoryEditError::UpsertPathUnreachable {
                requested,
                produced,
            } => {
                assert_eq!(requested, "zz");
                assert_eq!(produced, "b.zz");
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn upsert_at_a_reachable_path_reaches_a_fixed_point() {
        let op = MemoryEditOp::UpsertSection {
            path: "b.zz".into(),
            level: 2,
            heading: "ZZ".into(),
            body: "z".into(),
        };
        let once = apply_ops("# A\n\na\n\n# B\n\nb\n", std::slice::from_ref(&op)).expect("first");
        let twice = apply_ops(&once, std::slice::from_ref(&op)).expect("second");
        assert_eq!(once, twice);
        assert_eq!(once, "# A\n\na\n\n# B\n\nb\n\n## ZZ\n\nz\n");
    }

    /// Lines mmcp emits follow the document, so a CRLF body gains no LF islands from the heading and its separator.
    /// The supplied body is the author's text and goes in byte for byte:
    /// a memory may hold a fenced block documenting a protocol that mandates its own line endings.
    #[test]
    fn a_section_written_into_a_crlf_body_uses_crlf_for_generated_lines() {
        let out = apply_ops(
            "## A\r\n\r\na\r\n",
            &[MemoryEditOp::UpsertSection {
                path: "zz".into(),
                level: 2,
                heading: "ZZ".into(),
                body: "line1\nline2".into(),
            }],
        )
        .expect("apply");
        assert_eq!(out, "## A\r\n\r\na\r\n\r\n## ZZ\r\n\r\nline1\nline2\r\n");
    }

    #[test]
    fn a_fenced_block_keeps_the_line_endings_it_was_written_with() {
        let out = apply_ops(
            "## Protocol\n\nold\n",
            &[MemoryEditOp::ReplaceSectionBody {
                path: "protocol".into(),
                body: "```http\nGET / HTTP/1.1\r\nHost: x\r\n\r\n```".into(),
            }],
        )
        .expect("apply");
        assert_eq!(
            out,
            "## Protocol\n\n```http\nGET / HTTP/1.1\r\nHost: x\r\n\r\n```\n",
        );
    }

    /// A section body containing its own subheading is ordinary authoring.
    /// The reachability guard must identify the section it wrote by position, not by taking the document's last one.
    /// A heading in the body at the section's own level or shallower closes the section instead of nesting in it,
    /// so part of what was written falls outside the path.
    /// Accepting that let every repeat append another copy of the body.
    #[test]
    fn upsert_whose_body_escapes_the_section_is_rejected() {
        for escaping_body in ["# Top\n\nt", "## Peer\n\np"] {
            let err = apply_ops(
                "## A\n\na\n",
                &[MemoryEditOp::UpsertSection {
                    path: "zz".into(),
                    level: 2,
                    heading: "ZZ".into(),
                    body: escaping_body.into(),
                }],
            )
            .expect_err("must error");
            assert!(
                matches!(err, MemoryEditError::UpsertSectionNotSelfContained { .. }),
                "body {escaping_body:?} gave {err:?}",
            );
        }
    }

    /// A heading quoted inside a section is an example, not the start of the next section.
    /// Counting it as a peer cut every span short at that line,
    /// so a delete removed half a section and a move tore one in two, both without a word.
    const QUOTED_HEADING_SAMPLE: &str =
        "## A\n\nintro\n\n> quoted\n> ## InQuote\n\ntail of A\n\n## B\n\nb\n";

    #[test]
    fn deleting_a_section_removes_its_quoted_heading_too() {
        let out = apply_ops(
            QUOTED_HEADING_SAMPLE,
            &[MemoryEditOp::DeleteSection { path: "a".into() }],
        )
        .expect("apply");
        assert_eq!(out, "## B\n\nb\n");
    }

    #[test]
    fn moving_a_section_carries_its_quoted_heading_along() {
        let out = apply_ops(
            QUOTED_HEADING_SAMPLE,
            &[MemoryEditOp::MoveSectionAfter {
                target_path: "a".into(),
                anchor_path: "b".into(),
            }],
        )
        .expect("apply");
        assert_eq!(
            out,
            "## B\n\nb\n\n## A\n\nintro\n\n> quoted\n> ## InQuote\n\ntail of A\n\n",
        );
    }

    #[test]
    fn replacing_a_body_spans_past_a_quoted_heading() {
        let out = apply_ops(
            QUOTED_HEADING_SAMPLE,
            &[MemoryEditOp::ReplaceSectionBody {
                path: "a".into(),
                body: "new".into(),
            }],
        )
        .expect("apply");
        assert_eq!(out, "## A\n\nnew\n\n## B\n\nb\n");
    }

    #[test]
    fn upsert_accepts_a_body_whose_heading_sits_in_a_container() {
        for contained_body in [
            "> quoted\n> ## InQuote\n\nafter",
            "- item\n  ## InList\n\nafter",
        ] {
            let out = apply_ops(
                "## A\n\na\n",
                &[MemoryEditOp::UpsertSection {
                    path: "zz".into(),
                    level: 2,
                    heading: "ZZ".into(),
                    body: contained_body.into(),
                }],
            )
            .unwrap_or_else(|err| panic!("body {contained_body:?} rejected: {err:?}"));
            assert!(out.contains("## ZZ\n\n"), "got:\n{out}");
        }
    }

    /// Text closing on a bare carriage return still opens a new line
    /// for what follows; treating the return as a finished line welded
    /// the next heading onto it.
    #[test]
    fn a_heading_after_a_bare_carriage_return_keeps_its_blank_line() {
        let out = apply_ops(
            "intro\r",
            &[MemoryEditOp::UpsertSection {
                path: "zz".into(),
                level: 2,
                heading: "ZZ".into(),
                body: "z".into(),
            }],
        )
        .expect("apply");
        assert_eq!(out, "intro\r\n\n## ZZ\n\nz\n");
    }

    #[test]
    fn a_body_ending_in_a_carriage_return_is_still_terminated() {
        let out = apply_ops(
            "## A\n\nold\n",
            &[MemoryEditOp::ReplaceSectionBody {
                path: "a".into(),
                body: "x\r".into(),
            }],
        )
        .expect("apply");
        assert_eq!(out, "## A\n\nx\r\n");
    }

    #[test]
    fn upsert_whose_body_leaves_a_fence_open_is_rejected() {
        let err = apply_ops(
            "## A\n\n```\nx\n",
            &[MemoryEditOp::UpsertSection {
                path: "zz".into(),
                level: 2,
                heading: "ZZ".into(),
                body: "z".into(),
            }],
        )
        .expect_err("must error");
        assert!(matches!(
            err,
            MemoryEditError::UpsertSectionNotSelfContained { .. }
        ));
    }

    #[test]
    fn upsert_accepts_a_body_that_carries_a_subheading() {
        let out = apply_ops(
            "## A\n\na\n",
            &[MemoryEditOp::UpsertSection {
                path: "zz".into(),
                level: 2,
                heading: "ZZ".into(),
                body: "intro\n\n### Detail\n\nd".into(),
            }],
        )
        .expect("apply");
        assert_eq!(out, "## A\n\na\n\n## ZZ\n\nintro\n\n### Detail\n\nd\n");
    }

    /// Replacing a section at a level that renests it changes the path the caller must use next.
    /// Silently accepting that on the replace branch while rejecting it on the append branch
    /// made one condition behave two ways.
    #[test]
    fn upsert_that_would_renest_an_existing_section_is_rejected() {
        let err = apply_ops(
            "## A\n\na\n\n## B\n\nb\n",
            &[MemoryEditOp::UpsertSection {
                path: "b".into(),
                level: 3,
                heading: "B".into(),
                body: "b".into(),
            }],
        )
        .expect_err("must error");
        match err {
            MemoryEditError::UpsertPathUnreachable {
                requested,
                produced,
            } => {
                assert_eq!(requested, "b");
                assert_eq!(produced, "a.b");
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn a_move_round_trip_preserves_author_blank_lines() {
        let body = "## A\n\na\n\n\n\n## B\n\nb\n";
        let out = apply_ops(
            body,
            &[
                MemoryEditOp::MoveSectionAfter {
                    target_path: "a".into(),
                    anchor_path: "b".into(),
                },
                MemoryEditOp::MoveSectionBefore {
                    target_path: "a".into(),
                    anchor_path: "b".into(),
                },
            ],
        )
        .expect("apply");
        assert_eq!(out, body);
    }

    #[test]
    fn a_no_op_deletion_changes_nothing() {
        let body = "a\n\n\n\n";
        let out =
            apply_ops(body, &[MemoryEditOp::DeleteLines { start: 0, end: 0 }]).expect("apply");
        assert_eq!(out, body);
    }

    #[test]
    fn trailing_content_inside_an_unclosed_fence_survives() {
        let body = "## Code\n\n```text\nline1\n\n\n";
        let out = apply_ops(
            body,
            &[MemoryEditOp::InsertAtLine {
                line: 0,
                content: "X".into(),
            }],
        )
        .expect("apply");
        assert_eq!(out, "X\n\n## Code\n\n```text\nline1\n\n\n");
    }

    #[test]
    fn a_crlf_document_keeps_its_terminator_in_untouched_regions() {
        let body = "## A\r\n\r\nx\r\n";
        let out = apply_ops(
            body,
            &[MemoryEditOp::InsertAtLine {
                line: 0,
                content: "X".into(),
            }],
        )
        .expect("apply");
        assert_eq!(out, "X\r\n\r\n## A\r\n\r\nx\r\n");
    }

    #[test]
    fn a_move_and_its_inverse_restore_the_original_body() {
        let body = "## A\n\na\n\n## B\n\nb\n";
        let out = apply_ops(
            body,
            &[
                MemoryEditOp::MoveSectionAfter {
                    target_path: "a".into(),
                    anchor_path: "b".into(),
                },
                MemoryEditOp::MoveSectionBefore {
                    target_path: "a".into(),
                    anchor_path: "b".into(),
                },
            ],
        )
        .expect("apply");
        assert_eq!(out, body, "a round trip must not drift");
    }

    #[test]
    fn repeating_an_upsert_reaches_a_fixed_point() {
        let op = MemoryEditOp::UpsertSection {
            path: "references".into(),
            level: 2,
            heading: "References".into(),
            body: "link one".into(),
        };
        let once = apply_ops(SAMPLE, std::slice::from_ref(&op)).expect("first");
        let twice = apply_ops(&once, std::slice::from_ref(&op)).expect("second");
        assert_eq!(once, twice, "an idempotent op must not drift the body");
    }

    #[test]
    fn every_op_leaves_the_body_closed_by_one_terminator() {
        let out = apply_ops(
            "## Need\n\nneed body",
            &[MemoryEditOp::InsertSectionAfter {
                anchor_path: "need".into(),
                level: 2,
                heading: "Done".into(),
                body: "done body".into(),
            }],
        )
        .expect("apply");
        assert!(out.ends_with("done body\n"), "got:\n{out:?}");
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
        // Compose: insert a new section, then move an existing one before it.
        // Exercises the re-parse-per-op invariant.
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
