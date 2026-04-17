//! Section-aware parser and renderer for memory bodies (FR-026).
//!
//! A memory body is freeform CommonMark. This module slices it
//! into an ordered, flat sequence of addressable [`Section`]
//! entries — one per markdown heading — plus a leading "preamble"
//! span for any content before the first heading. The parser is a
//! thin layer over `pulldown-cmark` so edge cases (fenced code
//! containing `#` on a line, setext headings, nested lists that
//! look like headings, trailing whitespace) don't confuse the
//! splitter.
//!
//! Every heading gets a stable path id derived from the
//! slugified heading trail from root to target. Duplicate
//! siblings disambiguate with `-2`, `-3`... suffixes on the last
//! segment, matching how markdown anchor id generators handle
//! collisions. Path examples:
//!
//! - `resolution` — H2 `## Resolution`.
//! - `resolution.non-goals` — H3 `### Non-goals` nested under it.
//! - `notes.notes-2` — second `### Notes` under the same parent.
//!
//! The companion renderer reconstructs the body verbatim for
//! untouched spans and splices mutated sections back in. Byte
//! identity is not guaranteed across a round trip when the parse
//! normalizes trailing whitespace on the terminal line, but the
//! surface is designed to stay stable enough that diff-based
//! commit inspection remains readable.
//!
//! Frontmatter is never touched by this module — it belongs to
//! `edit_memory`'s frontmatter-args path.

use std::collections::HashMap;

use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};

/// One addressable span of a memory body.
///
/// The span covers the heading line (if any) plus every line of
/// body content down to — but not including — the next heading
/// at the same or shallower level. A memory body that opens with
/// prose before any heading exposes that prose as a synthetic
/// `preamble` section at index 0 with `level = 0`; callers can
/// address it by the literal path `"preamble"`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    /// Stable dot-separated path of slugified heading texts from
    /// root to target. See the module docs for the disambiguation
    /// rule on repeated sibling names.
    pub path: String,

    /// Heading level, `1..=6` for real headings. `0` is reserved
    /// for the synthetic preamble section.
    pub level: u8,

    /// Raw heading text as written by the author, minus the
    /// leading `#`s and any surrounding whitespace. Empty string
    /// for the preamble.
    pub heading: String,

    /// Zero-based inclusive start line index into the body. The
    /// heading line itself is included.
    pub line_start: usize,

    /// Zero-based exclusive end line index into the body. Equals
    /// the start line of the next section, or the total line count
    /// when the section is the last one.
    pub line_end: usize,
}

impl Section {
    /// Reserved path for the synthetic pre-heading span.
    pub const PREAMBLE_PATH: &'static str = "preamble";
}

/// Parse errors are intentionally narrow — the parser never
/// rejects well-formed markdown. The only failure mode today is
/// an internal inconsistency between `pulldown-cmark`'s event
/// offsets and the raw line table; it exists as a distinct type
/// so future additions (strict-mode validation, etc.) have a
/// landing zone.
#[derive(Debug, thiserror::Error)]
pub enum BodyParseError {
    /// `pulldown-cmark` emitted a heading whose byte offset fell
    /// outside the body. Should never happen against real input;
    /// reported for diagnostic purposes only.
    #[error("heading offset {offset} is outside the {total}-byte body")]
    OffsetOutOfRange { offset: usize, total: usize },
}

/// Parse `body` into a flat ordered sequence of [`Section`]s.
///
/// Lines are numbered from zero. Empty inputs return a single
/// preamble section spanning the empty range `0..0`. Inputs with
/// no headings return one preamble covering the whole body.
pub fn parse_sections(body: &str) -> Result<Vec<Section>, BodyParseError> {
    let lines: Vec<&str> = split_lines(body);
    let total_lines = lines.len();

    // Collect every heading's byte offset (start of the heading
    // line), level, and text from pulldown-cmark's event stream.
    let mut headings: Vec<(usize, u8, String)> = Vec::new();
    let mut options = Options::empty();
    options.insert(Options::ENABLE_HEADING_ATTRIBUTES);
    let parser = Parser::new_ext(body, options).into_offset_iter();

    let mut current_heading: Option<(usize, u8, String)> = None;
    for (event, range) in parser {
        match event {
            Event::Start(Tag::Heading { level, .. }) => {
                current_heading = Some((range.start, heading_level_to_u8(level), String::new()));
            }
            Event::End(TagEnd::Heading(_)) => {
                if let Some(h) = current_heading.take() {
                    headings.push(h);
                }
            }
            Event::Text(text) | Event::Code(text) => {
                if let Some((_, _, acc)) = current_heading.as_mut() {
                    acc.push_str(&text);
                }
            }
            _ => {}
        }
    }

    // Convert heading byte offsets to line indices.
    let line_starts = compute_line_starts(body);
    let mut heading_lines: Vec<(usize, u8, String)> = Vec::with_capacity(headings.len());
    for (offset, level, text) in headings {
        let line = line_index_for_offset(&line_starts, offset, body.len())?;
        heading_lines.push((line, level, text.trim().to_string()));
    }

    // Build the section sequence. A synthetic preamble always
    // leads, covering any content before the first heading.
    let mut sections: Vec<Section> = Vec::with_capacity(heading_lines.len() + 1);
    let preamble_end = heading_lines.first().map_or(total_lines, |h| h.0);
    sections.push(Section {
        path: Section::PREAMBLE_PATH.to_string(),
        level: 0,
        heading: String::new(),
        line_start: 0,
        line_end: preamble_end,
    });

    // Assign stable path ids using a hierarchical stack of
    // parent slug segments. Each level tracks how many times a
    // given slug has appeared under its parent so duplicate
    // siblings get `-2`, `-3`... suffixes.
    let mut stack: Vec<(u8, String)> = Vec::new();
    let mut sibling_counts: HashMap<(Vec<String>, String), u32> = HashMap::new();

    for (idx, (line, level, text)) in heading_lines.iter().enumerate() {
        // Pop stack entries whose level is >= the current heading
        // — we've left their subtree.
        while let Some(&(top_level, _)) = stack.last() {
            if top_level >= *level {
                stack.pop();
            } else {
                break;
            }
        }

        let slug_base = slugify_heading(text);
        let parent_key: Vec<String> = stack.iter().map(|(_, s)| s.clone()).collect();
        let key = (parent_key.clone(), slug_base.clone());
        let count = sibling_counts
            .entry(key)
            .and_modify(|n| *n += 1)
            .or_insert(1);
        let segment = if *count == 1 {
            slug_base.clone()
        } else {
            format!("{slug_base}-{count}")
        };

        let mut full_path = parent_key;
        full_path.push(segment.clone());
        let path = full_path.join(".");

        let line_end = heading_lines
            .get(idx + 1)
            .map_or(total_lines, |next| next.0);

        sections.push(Section {
            path,
            level: *level,
            heading: text.clone(),
            line_start: *line,
            line_end,
        });

        stack.push((*level, segment));
    }

    Ok(sections)
}

/// Re-render a body from `raw` using `sections` as the slicing
/// plan. Each section span is emitted in order; the caller has
/// already arranged `sections` to reflect any insertions, moves,
/// or deletes. The preamble section is always section 0.
///
/// This is the pure-rendering counterpart to [`parse_sections`]
/// and does not know about edit operations. The op applier in
/// `mmcp-store::memory_ops` is what rearranges the sections
/// before calling in here.
#[must_use]
pub fn render_sections(raw: &str, sections: &[Section]) -> String {
    let lines = split_lines(raw);
    let mut out = String::with_capacity(raw.len());
    for (idx, section) in sections.iter().enumerate() {
        for line_idx in section.line_start..section.line_end {
            if let Some(line) = lines.get(line_idx) {
                out.push_str(line);
                out.push('\n');
            }
        }
        // Drop trailing newline after the final section so bodies
        // that originally lacked one don't gain one through the
        // round trip.
        if idx == sections.len() - 1 && !raw.ends_with('\n') {
            out.pop();
        }
    }
    out
}

/// Slugify a heading into a kebab-case token. Unicode-folded +
/// whitespace-collapsed, matching the `slug` crate's behaviour.
/// The leading-hyphen / trailing-hyphen corner cases are handled
/// by the crate itself.
#[must_use]
pub fn slugify_heading(text: &str) -> String {
    let base = slug::slugify(text.trim());
    if base.is_empty() {
        "section".to_string()
    } else {
        base
    }
}

fn heading_level_to_u8(level: HeadingLevel) -> u8 {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

fn split_lines(body: &str) -> Vec<&str> {
    // Keep byte indices aligned with `pulldown-cmark`'s offsets
    // by splitting on `\n` without swallowing empty trailing
    // lines. `lines()` drops a trailing `\n` so we use a manual
    // split.
    if body.is_empty() {
        return Vec::new();
    }
    let mut out: Vec<&str> = body.split('\n').collect();
    // `split` on a string ending with `\n` appends a trailing
    // empty element — drop it so line counts match human
    // expectation.
    if body.ends_with('\n') {
        out.pop();
    }
    out
}

fn compute_line_starts(body: &str) -> Vec<usize> {
    let mut starts = vec![0];
    for (i, b) in body.bytes().enumerate() {
        if b == b'\n' {
            starts.push(i + 1);
        }
    }
    starts
}

fn line_index_for_offset(
    line_starts: &[usize],
    offset: usize,
    total: usize,
) -> Result<usize, BodyParseError> {
    if offset > total {
        return Err(BodyParseError::OffsetOutOfRange { offset, total });
    }
    Ok(line_starts
        .binary_search(&offset)
        .unwrap_or_else(|insertion| insertion.saturating_sub(1)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_body_yields_only_preamble() {
        let sections = parse_sections("").expect("parse");
        assert_eq!(sections.len(), 1);
        let preamble = &sections[0];
        assert_eq!(preamble.path, Section::PREAMBLE_PATH);
        assert_eq!(preamble.level, 0);
        assert_eq!(preamble.line_start, 0);
        assert_eq!(preamble.line_end, 0);
    }

    #[test]
    fn headingless_body_stays_in_preamble() {
        let body = "just a paragraph\nwith two lines";
        let sections = parse_sections(body).expect("parse");
        assert_eq!(sections.len(), 1);
        let preamble = &sections[0];
        assert_eq!(preamble.path, Section::PREAMBLE_PATH);
        assert_eq!(preamble.line_start, 0);
        assert_eq!(preamble.line_end, 2);
    }

    #[test]
    fn flat_h2_sections_assign_slugified_paths() {
        let body = "\
## Need

prose

## Resolution

more prose
";
        let sections = parse_sections(body).expect("parse");
        assert_eq!(sections.len(), 3, "preamble + two h2 sections");
        assert_eq!(sections[1].path, "need");
        assert_eq!(sections[1].level, 2);
        assert_eq!(sections[1].heading, "Need");
        assert_eq!(sections[2].path, "resolution");
    }

    #[test]
    fn nested_h3_gets_dotted_path() {
        let body = "\
## Resolution

prose

### Non-goals

bullets
";
        let sections = parse_sections(body).expect("parse");
        assert_eq!(sections.len(), 3);
        assert_eq!(sections[1].path, "resolution");
        assert_eq!(sections[2].path, "resolution.non-goals");
        assert_eq!(sections[2].level, 3);
    }

    #[test]
    fn duplicate_sibling_headings_get_numeric_suffix() {
        let body = "\
## Resolution

### Notes

first notes

### Notes

second notes
";
        let sections = parse_sections(body).expect("parse");
        let paths: Vec<_> = sections.iter().map(|s| s.path.as_str()).collect();
        assert_eq!(
            paths,
            vec![
                "preamble",
                "resolution",
                "resolution.notes",
                "resolution.notes-2"
            ],
        );
    }

    #[test]
    fn fenced_code_hashes_are_not_headings() {
        let body = "\
## Need

```rust
// # not a heading
fn f() {}
```

## Real heading

body
";
        let sections = parse_sections(body).expect("parse");
        // preamble + the two real H2s; the fenced line must not
        // spawn a section.
        let paths: Vec<_> = sections.iter().map(|s| s.path.as_str()).collect();
        assert_eq!(paths, vec!["preamble", "need", "real-heading"]);
    }

    #[test]
    fn section_spans_cover_all_body_lines() {
        let body = "intro\n\n## A\n\nalpha\n\n## B\n\nbeta\n";
        let sections = parse_sections(body).expect("parse");
        // Every line in the body must fall inside exactly one
        // section span. The preamble covers lines 0..2 (intro +
        // blank), then "## A" opens section A.
        let total_lines = sections.last().map(|s| s.line_end).unwrap_or(0);
        let mut coverage = vec![false; total_lines];
        for section in &sections {
            for line in section.line_start..section.line_end {
                assert!(
                    !coverage[line],
                    "line {line} is in two sections ({} and ...)",
                    section.path
                );
                coverage[line] = true;
            }
        }
        assert!(coverage.iter().all(|&c| c), "every line must be covered");
    }

    #[test]
    fn render_round_trip_preserves_body_for_untouched_parse() {
        let body = "preface\n\n## Need\n\nbody\n\n### Sub\n\ndetails\n\n## Done\n\nwrap up\n";
        let sections = parse_sections(body).expect("parse");
        let rendered = render_sections(body, &sections);
        assert_eq!(rendered, body);
    }

    #[test]
    fn render_trailing_newline_is_preserved_as_is() {
        let without = "## A\nbody without trailing newline";
        let sections = parse_sections(without).expect("parse");
        let rendered = render_sections(without, &sections);
        assert_eq!(rendered, without);

        let with = "## A\nbody with trailing newline\n";
        let sections = parse_sections(with).expect("parse");
        let rendered = render_sections(with, &sections);
        assert_eq!(rendered, with);
    }

    #[test]
    fn empty_heading_slug_falls_back_to_section() {
        // Contrived: a heading that slugifies to empty (e.g. just
        // punctuation). The slug helper replaces it with
        // "section" so the path stays non-empty.
        assert_eq!(slugify_heading("???"), "section");
        assert_eq!(slugify_heading("   "), "section");
    }
}
