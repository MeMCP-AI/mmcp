//! Seam-preserving byte-range splice for memory bodies.
//!
//! Every body mutation reduces to `prefix + fragment + tail`.
//! Concatenating the three runs raw corrupts the document whenever a
//! seam does not already carry the separator markdown expects.
//!
//! Line seam: a prefix not terminated by a newline swallows the
//! fragment onto its last line, and a fragment not terminated by a
//! newline swallows the tail's first line.
//! Memory bodies are stored without a trailing newline, so every
//! insertion at end of body hits this.
//!
//! Block seam: a heading written flush against an adjacent paragraph
//! line reads as a wall of text and hides the section boundary the
//! path-addressed editing surface exposes.
//! A heading therefore keeps a blank line on whichever side a splice
//! touches.
//!
//! End-of-document seam: the result always closes with exactly one
//! line terminator.
//! Without that rule an append leaves the document unterminated and a
//! move drags its source section's trailing blank line to the tail,
//! so repeating an edit keeps changing the committed bytes.
//!
//! A heading construct opens and closes on different lines, so the
//! two sides are tracked separately.
//! A setext heading opens on its text line and closes on its
//! underline: content ending before the text line needs a blank line
//! or it merges into the heading, while the gap between text and
//! underline must stay closed or the heading falls apart.
//! Conflating the two either splits headings or welds text into them.
//!
//! A heading nested in a block quote or a list item is excluded: a
//! blank line there terminates the container and splits one block
//! into two.
//! Nesting comes from [`parse_sections`], the only component that
//! knows a `#` line inside a fence or a quote is not a top-level
//! heading.

use std::ops::Range;

use super::body::{BodyParseError, parse_sections};

/// Failure modes of [`splice`].
///
/// The range bounds come from a caller's section span or line index,
/// so a violation is a caller bug rather than bad user input; each
/// variant carries the offending offset so the caller can report
/// which computation produced it.
#[derive(Debug, thiserror::Error)]
pub enum SpliceError {
    /// Range bounds are reversed.
    #[error("splice range start {start} is past its end {end}")]
    InvertedRange { start: usize, end: usize },

    /// A range bound points past the end of the body.
    #[error("splice offset {offset} is past the end of the {length}-byte body")]
    OutOfBounds { offset: usize, length: usize },

    /// A range bound falls inside a multi-byte character.
    #[error("splice offset {offset} is not a character boundary")]
    NotCharBoundary { offset: usize },

    /// The section parser rejected the body or the fragment.
    #[error(transparent)]
    Parse(#[from] BodyParseError),
}

/// @brief Replace `range` in `body` with `fragment`, normalizing every seam.
///
/// Both bounds must sit on a line start or at end of body; callers
/// derive them from a section span or a line index, which satisfies
/// that by construction. A bound elsewhere is not rejected, but it
/// splits a line, and a bound inside a CRLF pair leaves a stray
/// terminator on each side.
///
/// The range is replaced exactly. Nothing outside it is removed, so
/// deleting the last section can leave the blank line that separated
/// it; a relocation trims that at its own layer, where the intent to
/// move rather than delete is known.
///
/// `fragment` is spliced in verbatim. Its interior line endings are
/// the author's bytes, and a memory can legitimately hold a fenced
/// block documenting a protocol that mandates CRLF, so only the
/// terminators this function itself emits follow the document's.
///
/// @param body Current markdown body.
/// @param range Byte range to replace; an empty range inserts.
/// @param fragment Replacement text; an empty fragment deletes.
/// @return Rewritten body, its last line always terminated.
pub fn splice(body: &str, range: Range<usize>, fragment: &str) -> Result<String, SpliceError> {
    validate_range(body, &range)?;

    let terminator = line_terminator(body);
    let body_headings = HeadingSeams::scan(body)?;
    let fragment_headings = HeadingSeams::scan(fragment)?;

    let prefix = &body[..range.start];
    let tail = &body[range.end..];

    let prefix_closes_on_heading = body_headings.closes_at(last_line_start(prefix));
    let tail_opens_on_heading = body_headings.opens.contains(&range.end);
    let fragment_opens_on_heading = fragment_headings.opens.contains(&0);
    let fragment_closes_on_heading = fragment_headings.closes_at(last_line_start(fragment));

    let mut out = String::with_capacity(prefix.len() + fragment.len() + tail.len() + 4);
    out.push_str(prefix);

    if !fragment.is_empty() {
        push_seam(
            &mut out,
            prefix_closes_on_heading || fragment_opens_on_heading,
            terminator,
        );
        out.push_str(fragment);
    }

    if !tail.is_empty() {
        // An empty fragment collapses the two seams into one, so the
        // left side of the remaining seam is the prefix itself.
        let left_closes_on_heading = if fragment.is_empty() {
            prefix_closes_on_heading
        } else {
            fragment_closes_on_heading
        };
        push_seam(
            &mut out,
            left_closes_on_heading || tail_opens_on_heading,
            terminator,
        );
        out.push_str(tail);
    }

    close_document(&mut out, terminator);
    Ok(out)
}

/// @brief Close the current line, completing a dangling carriage return rather than doubling it.
///
/// Text ending in a lone carriage return already opens a CRLF pair, so
/// a full terminator there would emit `\r\r\n`.
///
/// The line table counts only newlines, so a bare carriage return is
/// not a line ending anywhere else in this crate; treating it as one
/// here would leave a heading welded to the text above it.
fn terminate_line(out: &mut String, terminator: &str) {
    if out.ends_with('\n') {
        return;
    }
    if out.ends_with('\r') {
        out.push('\n');
    } else {
        out.push_str(terminator);
    }
}

/// @brief Reject a range that cannot index `body`.
fn validate_range(body: &str, range: &Range<usize>) -> Result<(), SpliceError> {
    if range.start > range.end {
        return Err(SpliceError::InvertedRange {
            start: range.start,
            end: range.end,
        });
    }
    for offset in [range.start, range.end] {
        if offset > body.len() {
            return Err(SpliceError::OutOfBounds {
                offset,
                length: body.len(),
            });
        }
        if !body.is_char_boundary(offset) {
            return Err(SpliceError::NotCharBoundary { offset });
        }
    }
    Ok(())
}

/// @brief Close the current line, adding a blank line when the seam abuts a heading.
///
/// A seam at the very start of the body needs no separator.
/// Both pushes are idempotent so an already-well-formed seam is left
/// byte-identical.
fn push_seam(out: &mut String, blank_line: bool, terminator: &str) {
    if out.is_empty() {
        return;
    }
    terminate_line(out, terminator);
    if blank_line && !ends_with_blank_line(out) {
        out.push_str(terminator);
    }
}

/// @brief Terminate the document's last line when it is left open.
///
/// Purely additive. Trailing blank lines are content the author
/// wrote, and a trailing carriage return can be fenced data, so
/// trimming a run here would delete text no op asked to touch.
fn close_document(out: &mut String, terminator: &str) {
    if !out.is_empty() {
        terminate_line(out, terminator);
    }
}

/// @brief Whether `text` closes with an empty line.
///
/// Recognizes both terminator styles so a CRLF document is not given
/// a second blank line on every edit.
fn ends_with_blank_line(text: &str) -> bool {
    let Some(head) = text.strip_suffix('\n') else {
        return false;
    };
    let head = head.strip_suffix('\r').unwrap_or(head);
    head.is_empty() || head.ends_with('\n')
}

/// @brief Prevailing line terminator of `text`, by majority.
///
/// Callers rendering markdown to splice in use this so the lines they
/// generate match the document instead of seeding it with islands of
/// the other style.
///
/// Sampling a single newline misreads a mixed document and misreads
/// any document whose first byte is a newline, so every terminator is
/// counted. Ties go to CRLF, which only arises when the two styles
/// are already equally present.
pub fn line_terminator(text: &str) -> &'static str {
    let bytes = text.as_bytes();
    let mut crlf = 0usize;
    let mut lf = 0usize;
    for (index, byte) in bytes.iter().enumerate() {
        if *byte != b'\n' {
            continue;
        }
        if index > 0 && bytes[index - 1] == b'\r' {
            crlf += 1;
        } else {
            lf += 1;
        }
    }
    if crlf > 0 && crlf >= lf { "\r\n" } else { "\n" }
}

/// Line starts of every top-level heading in a text, split by which
/// side of the construct they sit on.
///
/// The two sets differ only for setext headings, whose text line
/// opens the construct and whose underline closes it.
struct HeadingSeams {
    /// Byte offset of each heading's first line.
    opens: Vec<usize>,

    /// Byte offset of each heading's last line.
    closes: Vec<usize>,
}

impl HeadingSeams {
    /// @brief Collect the heading seams of `text`.
    ///
    /// The synthetic preamble carries level zero and is skipped; it
    /// has no heading line of its own. A heading enclosed in a block
    /// quote or a list item is skipped too, because separating it
    /// would break the container rather than the heading.
    fn scan(text: &str) -> Result<Self, SpliceError> {
        let line_offsets = line_start_offsets(text);
        let mut seams = Self {
            opens: Vec::new(),
            closes: Vec::new(),
        };
        for section in parse_sections(text)?
            .iter()
            .filter(|section| section.level > 0 && section.container_depth == 0)
        {
            let last_heading_line = section.heading_line_end.saturating_sub(1);
            if let Some(&offset) = line_offsets.get(section.line_start) {
                seams.opens.push(offset);
            }
            if let Some(&offset) = line_offsets.get(last_heading_line) {
                seams.closes.push(offset);
            }
        }
        Ok(seams)
    }

    /// @brief Whether `line_start` is the last line of a heading construct.
    fn closes_at(&self, line_start: Option<usize>) -> bool {
        line_start.is_some_and(|offset| self.closes.contains(&offset))
    }
}

/// @brief Byte offset of every line start in `text`, indexed like the body parser's line table.
///
/// A trailing newline closes the last line rather than opening an
/// empty one, matching how the parser counts lines.
fn line_start_offsets(text: &str) -> Vec<usize> {
    let mut offsets = vec![0];
    for (index, byte) in text.bytes().enumerate() {
        if byte == b'\n' && index + 1 < text.len() {
            offsets.push(index + 1);
        }
    }
    offsets
}

/// @brief Byte offset where the final line of `text` begins, or `None` when `text` is empty.
///
/// A trailing newline is dropped first so the final line is the last
/// line carrying content, not the empty span behind it.
fn last_line_start(text: &str) -> Option<usize> {
    if text.is_empty() {
        return None;
    }
    let without_terminator = text.strip_suffix('\n').unwrap_or(text);
    Some(without_terminator.rfind('\n').map_or(0, |index| index + 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_at_end_of_unterminated_body_starts_a_new_line() {
        let body = "alpha\nbeta";
        let out = splice(body, body.len()..body.len(), "gamma").expect("splice");
        assert_eq!(out, "alpha\nbeta\ngamma\n");
    }

    #[test]
    fn insert_before_heading_keeps_a_blank_line() {
        let body = "intro\n\n## Need\n\nneed body\n";
        let heading_at = body.find("## Need").expect("heading present");
        let out = splice(body, heading_at..heading_at, "note").expect("splice");
        assert_eq!(out, "intro\n\nnote\n\n## Need\n\nneed body\n");
    }

    #[test]
    fn insert_after_heading_keeps_a_blank_line() {
        let body = "## Need\nneed body";
        let after_heading = body.find("need body").expect("body present");
        let out = splice(body, after_heading..after_heading, "inserted").expect("splice");
        assert_eq!(out, "## Need\n\ninserted\nneed body\n");
    }

    #[test]
    fn heading_fragment_gets_a_blank_line_on_both_sides() {
        let body = "intro\ntail line";
        let seam = body.find("tail line").expect("tail present");
        let out = splice(body, seam..seam, "## New").expect("splice");
        assert_eq!(out, "intro\n\n## New\n\ntail line\n");
    }

    #[test]
    fn well_formed_seams_are_left_byte_identical() {
        let body = "intro\n\n## Need\n\nneed body\n\n## Done\n\ndone body\n";
        let start = body.find("## Done").expect("heading present");
        let out = splice(body, start..start, "## Extra\n\nextra body").expect("splice");
        assert_eq!(
            out,
            "intro\n\n## Need\n\nneed body\n\n## Extra\n\nextra body\n\n## Done\n\ndone body\n",
        );
    }

    #[test]
    fn deletion_seam_does_not_merge_the_two_sides() {
        let body = "alpha\nbeta\ngamma";
        let start = body.find("beta").expect("beta present");
        let end = body.find("gamma").expect("gamma present");
        let out = splice(body, start..end, "").expect("splice");
        assert_eq!(out, "alpha\ngamma\n");
    }

    #[test]
    fn hash_line_inside_a_fence_is_not_treated_as_a_heading() {
        let body = "```rust\n// # not a heading\n```";
        let fence_body = body.find("// #").expect("fenced line present");
        let out = splice(body, fence_body..fence_body, "// inserted").expect("splice");
        assert_eq!(out, "```rust\n// inserted\n// # not a heading\n```\n");
    }

    #[test]
    fn splice_into_an_empty_body_emits_the_fragment_alone() {
        let out = splice("", 0..0, "first line").expect("splice");
        assert_eq!(out, "first line\n");
    }

    #[test]
    fn fragment_before_a_leading_heading_does_not_open_with_a_blank_line() {
        let body = "## First\n\nbody";
        let out = splice(body, 0..0, "preamble text").expect("splice");
        assert_eq!(out, "preamble text\n\n## First\n\nbody\n");
    }

    /// Closing the document only ever adds a terminator to an open
    /// last line. Blank runs are content and are never trimmed.
    #[test]
    fn result_never_leaves_the_last_line_open() {
        let body = "## A\n\na";
        let out = splice(body, body.len()..body.len(), "tail line").expect("splice");
        assert_eq!(out, "## A\n\na\ntail line\n");
    }

    #[test]
    fn trailing_blank_lines_are_content_and_survive() {
        let body = "## A\n\na\n\n\n\n";
        let out = splice(body, body.len()..body.len(), "tail line\n\n\n").expect("splice");
        assert_eq!(out, "## A\n\na\n\n\n\ntail line\n\n\n");
    }

    #[test]
    fn splicing_twice_reaches_a_fixed_point() {
        let body = "## A\n\na\n\n## B\n\nb\n";
        let once = splice(body, 0..0, "intro").expect("first");
        let twice = splice(&once, 0..0, "intro").expect("second");
        assert_eq!(twice, "intro\nintro\n\n## A\n\na\n\n## B\n\nb\n");
    }

    #[test]
    fn crlf_body_keeps_its_terminator_and_gains_no_extra_blank_line() {
        let body = "## A\r\n\r\nx\r\n\r\n## B\r\n\r\ny\r\n";
        let start = body.find("## B").expect("heading present");
        let out = splice(body, start..start, "note").expect("splice");
        assert_eq!(out, "## A\r\n\r\nx\r\n\r\nnote\r\n\r\n## B\r\n\r\ny\r\n");
    }

    #[test]
    fn setext_heading_is_not_split_by_the_blank_line_rule() {
        let body = "Title\n=====\n\nold body\n";
        let underline_at = body.find("=====").expect("underline present");
        let out = splice(body, underline_at..underline_at, "X").expect("splice");
        assert_eq!(
            out, "Title\nX\n=====\n\nold body\n",
            "a setext underline must never be pushed away from its text line",
        );
    }

    #[test]
    fn heading_inside_a_block_quote_is_not_split_from_its_container() {
        let body = "> intro\n> ## Quoted\n>\n> after\n";
        let quoted_at = body.find("> ## Quoted").expect("quoted heading present");
        let out = splice(body, quoted_at..quoted_at, "> note").expect("splice");
        assert_eq!(
            out, "> intro\n> note\n> ## Quoted\n>\n> after\n",
            "a blank line here would terminate the block quote",
        );
    }

    #[test]
    fn non_char_boundary_offset_is_rejected_rather_than_panicking() {
        let err = splice("héllo", 1..2, "X").expect_err("must reject");
        assert!(matches!(err, SpliceError::NotCharBoundary { offset: 2 }));
    }

    #[test]
    fn offset_past_the_body_is_rejected() {
        let err = splice("abc", 0..99, "X").expect_err("must reject");
        assert!(matches!(
            err,
            SpliceError::OutOfBounds {
                offset: 99,
                length: 3
            }
        ));
    }

    #[test]
    fn inverted_range_is_rejected() {
        // Built from bindings so the reversed-range lint, which only
        // inspects literals, does not reject the intended input.
        let (start, end) = (4, 2);
        let err = splice("abcdef", start..end, "X").expect_err("must reject");
        assert!(matches!(
            err,
            SpliceError::InvertedRange { start: 4, end: 2 }
        ));
    }
}
