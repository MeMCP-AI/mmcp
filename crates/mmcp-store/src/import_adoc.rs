//! AsciiDoc -> Markdown conversion used by the import pipeline.
//!
//! The project stores every memory as `.md` with a TOML/YAML frontmatter fence.
//! Supporting AsciiDoc on the input side is a one-way bridge:
//! an `.adoc`/`.asciidoc` file is parsed with `acdc-parser` and rendered to CommonMark,
//! via `acdc-converters-markdown`, then handed verbatim to [`crate::memory::import_memory`],
//! as if the operator had dropped a `.md` file.
//! Nothing else in the store layer needs to know the source format:
//! on-disk storage, diagnostics, and the MCP read surface all stay markdown-native.
//!
//! `acdc-converters-markdown` has two known weak spots that real-world Git docs trigger constantly:
//! it drops description-list bodies (`label::\n\tbody`) on the floor with only a warning comment,
//! and it doesn't render `linkgit:foo[N]` macros cleanly,
//! (escaping the brackets and leaving a literal `linkgit:` prefix).
//! It also leaves a sprinkling of `<!-- Warning: ... -->` HTML comments behind.
//! The `setext` parser feature also has to be enabled by hand,
//! for the `~~~~~`/`^^^^^` underline styles to parse as headings:
//! the upstream default is off.
//!
//! Rather than forking the converter, this module wraps it:
//! a small pre-processor rewrites the constructs acdc can't faithfully render into ones it can,
//! (see `preprocess_adoc`), the converter does the AST -> markdown translation,
//! and `postprocess_markdown` strips warning comments and the `linkgit:` macro residue.
//! Each step is independently testable.
//!
//! `DocumentAttributes` on the parsed AsciiDoc side are NOT currently promoted into the memory's TOML frontmatter.
//! Operators who want structured frontmatter must either pre-embed it in the source,
//! (AsciiDoc supports front-matter-style preambles that `gray_matter` will pick up after conversion),
//! or supply [`crate::memory::SynthFrontmatter`] via the import CLI flags.

use acdc_converters_core::{Converter, Diagnostics, Options as ConverterOptions};
use acdc_converters_markdown::{MarkdownVariant, Processor};
use acdc_parser::{Options as ParserOptions, Parser};
use thiserror::Error;

/// File extensions the import pipeline routes through
/// [`convert_adoc_to_markdown`].
///
/// Case-insensitive; operators on Windows sometimes see `.ADOC` from
/// editors that upper-case extensions, so the matcher in
/// [`is_adoc_filename`] normalises before comparing.
pub const ADOC_EXTENSIONS: &[&str] = &["adoc", "asciidoc"];

/// True when `filename` looks like an AsciiDoc source.
///
/// Matches on the last `.`-separated segment of the name,
/// so paths are fine as long as the caller hands the full base name (not a stripped stem).
/// Empty string and bare `.adoc` (no stem) both return false:
/// a memory slug must survive the stem-extraction in [`crate::memory::slugify_filename`].
#[must_use]
pub fn is_adoc_filename(filename: &str) -> bool {
    // A bare `.adoc` has an empty stem - nothing survives to become
    // a memory slug, so reject it alongside ".", "trailing.", and
    // extensionless names.
    let Some((stem, ext)) = filename.rsplit_once('.') else {
        return false;
    };
    if stem.is_empty() || ext.is_empty() {
        return false;
    }
    let lower = ext.to_ascii_lowercase();
    ADOC_EXTENSIONS.iter().any(|e| *e == lower)
}

/// Structured failures emitted by [`convert_adoc_to_markdown`].
///
/// The import entry point maps these to CLI errors carrying the
/// original file path so operators can locate the offending source.
#[derive(Debug, Error)]
pub enum AdocConvertError {
    #[error("failed to parse AsciiDoc source: {0}")]
    Parse(String),

    #[error("failed to render AsciiDoc AST to markdown: {0}")]
    Render(String),

    #[error("markdown renderer produced non-UTF8 output: {0}")]
    Utf8(String),
}

/// Parse `source` as AsciiDoc and render it to CommonMark.
///
/// CommonMark is chosen over GFM because mmcp's existing body-section
/// parser (`mmcp_core::memory::body`) is built on `pulldown-cmark` in
/// its default CommonMark mode; staying aligned keeps the round trip
/// between imported adoc content and semantic section addressing
/// coherent.
///
/// Pre/post-processing is layered around the upstream converter to recover content
/// acdc would otherwise drop or mangle (description lists, `linkgit:` macros, warning comments).
/// See the module-level docs for the rationale and `tests` for the regression coverage.
pub fn convert_adoc_to_markdown(source: &str) -> Result<String, AdocConvertError> {
    let normalized = preprocess_adoc(source);
    let parser_options = ParserOptions::builder().with_setext().build();
    let parsed = Parser::new(&normalized)
        .with_options(parser_options)
        .parse()
        .map_err(|e| AdocConvertError::Parse(e.to_string()))?;
    let processor = Processor::new(ConverterOptions::default(), Default::default())
        .with_variant(MarkdownVariant::CommonMark);
    let mut out = Vec::new();
    let warning_source = processor.warning_source();
    let mut warnings = Vec::new();
    let mut diagnostics = Diagnostics::new(&warning_source, &mut warnings);
    processor
        .write_to(parsed.document(), &mut out, None, None, &mut diagnostics)
        .map_err(|e| AdocConvertError::Render(e.to_string()))?;
    let raw = String::from_utf8(out).map_err(|e| AdocConvertError::Utf8(e.to_string()))?;
    Ok(postprocess_markdown(&raw))
}

/// Rewrite AsciiDoc constructs that `acdc-converters-markdown` can't
/// faithfully render into ones it can.
///
/// Currently handles:
/// - **Description lists** (`label::\n\tbody`): rewritten as a bold label paragraph followed by the indented body.
///   The upstream visitor drops `DescriptionList` content unconditionally (`visit_description_list` is a TODO);
///   rewriting on the input side is the only way to preserve the body without forking the converter.
///   The rewrite is line-oriented and skips lines inside listing/literal/comment delimiters (`----`, `....`, `////`),
///   so it doesn't touch content the user explicitly fenced.
fn preprocess_adoc(source: &str) -> String {
    let mut out = String::with_capacity(source.len() + 64);
    // The active listing-block delimiter character, if currently inside one.
    // AsciiDoc accepts variable-length delimiters (`----`, `--------`, `------------`),
    // so only the character is tracked; the close just needs >=4 of the same character on a line by itself.
    let mut listing_char: Option<u8> = None;
    let lines: Vec<&str> = source.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim_end();
        // Already inside a listing block? Check for the closing delimiter (any-length run of the same char).
        if let Some(ch) = listing_char {
            if is_delimiter_line(trimmed, ch) {
                listing_char = None;
            }
            out.push_str(line);
            out.push('\n');
            i += 1;
            continue;
        }
        // Not in a listing block.
        // Decide whether this line OPENS one.
        // AsciiDoc only treats a `----` (or `....`/`////`/`++++`) line as a listing-block opener,
        // when it stands ALONE, i.e. the previous line is blank, a block attribute, or the start of the file.
        // When the previous line is non-blank content, the `----` is a setext-style section-heading underline,
        // and must NOT trigger listing mode.
        // (acdc's `setext` feature handles the underline itself; this check simply avoids interfering.)
        if let Some(ch) = detect_listing_opener(&lines, i) {
            listing_char = Some(ch);
            out.push_str(line);
            out.push('\n');
            i += 1;
            continue;
        }
        // Description-list detection.
        // The label MUST NOT contain `::` itself (outside backticks), must be non-empty,
        // and must not look like a code attribute (e.g. `[source,rust]`).
        // Greedily consumes every continuation line that belongs to the same item:
        // single-line form, two-line form, and multi-paragraph bodies separated by `+`.
        if let Some(rewrite) = description_list_rewrite(&lines, i) {
            out.push_str(&rewrite.rewritten);
            i += rewrite.consumed;
            continue;
        }
        out.push_str(line);
        out.push('\n');
        i += 1;
    }
    out
}

/// True when `line` is a delimiter run: >=4 copies of `ch`, with no other characters.
/// Used both to recognise listing-block openers (`----`, `--------`, ...) and their closers,
/// regardless of whether the lengths match exactly.
fn is_delimiter_line(line: &str, ch: u8) -> bool {
    let bytes = line.as_bytes();
    bytes.len() >= 4 && bytes.iter().all(|b| *b == ch)
}

/// Return the delimiter character if `lines[i]` opens a listing/literal/comment/passthrough block.
/// Returns `None` when the line is a setext heading underline (previous line has content).
fn detect_listing_opener(lines: &[&str], i: usize) -> Option<u8> {
    const OPENERS: &[u8] = b"-./+";
    let trimmed = lines[i].trim_end();
    let bytes = trimmed.as_bytes();
    let first = *bytes.first()?;
    if !OPENERS.contains(&first) {
        return None;
    }
    if !is_delimiter_line(trimmed, first) {
        return None;
    }
    // Setext underlines: the previous line is non-blank content.
    // Treat the delimiter as a heading underline, not a block opener.
    // Also defers to acdc when the previous line is a block attribute on its own (`[verse]`):
    // legal but rare enough to conservatively let acdc handle it,
    // since [...] attributes always introduce blocks anyway.
    let prev = if i == 0 { "" } else { lines[i - 1].trim_end() };
    if !(prev.is_empty() || prev.starts_with('[') && prev.ends_with(']')) {
        return None;
    }
    Some(first)
}

struct DescriptionListRewrite {
    rewritten: String,
    consumed: usize,
}

fn description_list_rewrite(lines: &[&str], start: usize) -> Option<DescriptionListRewrite> {
    let line = lines[start];
    let trimmed = line.trim_end_matches([' ', '\t']);
    let (idx, marker_len) = find_dlist_marker(trimmed)?;
    let label = &trimmed[..idx];
    let rest = &trimmed[idx + marker_len..];
    let label_t = label.trim();
    if label_t.is_empty() || is_attribute_line(label) {
        return None;
    }
    let inline_body = rest.trim_start();
    let mut body_lines: Vec<String> = Vec::new();
    if !inline_body.is_empty() {
        body_lines.push(inline_body.to_string());
    }
    // Walk forward absorbing the full continuation block.
    // AsciiDoc description-list items extend across:
    //  - indented continuation lines (leading tab / 2+ spaces),
    //  - `+` lines that bridge paragraphs within the item,
    //  - blocks attached after a `+` continuation, regardless of
    //    indent (`. ordered lists`, `* bullet lists`, free
    //    paragraphs, the `+` makes them part of the item),
    //  - single blank lines between content.
    // The block terminates at one of these unambiguous boundaries:
    //  - a `LABEL::` line at column 0 (next item),
    //  - an AsciiDoc section heading (`=`, `==`, `===`, `====`,
    //    `=====`) at column 0,
    //  - a top-level block-attribute line (`[…]`) at column 0,
    //  - two consecutive blank lines,
    //  - end of file.
    let mut j = start + 1;
    let mut consecutive_blanks = 0;
    let mut after_plus = false;
    while j < lines.len() {
        let candidate = lines[j];
        let trimmed_c = candidate.trim_end();
        if trimmed_c.is_empty() {
            consecutive_blanks += 1;
            if consecutive_blanks >= 2 {
                // Two blanks in a row signal the end of the list item even after a `+`:
                // AsciiDoc only attaches ONE block to a `+` continuation.
                break;
            }
            // A blank line ends the after-plus protection: any
            // following non-indented content needs a fresh `+`.
            after_plus = false;
            body_lines.push(String::new());
            j += 1;
            continue;
        }
        let starts_indented = candidate.starts_with('\t') || candidate.starts_with("  ");
        let is_plus = trimmed_c == "+";
        // Hard terminators at column 0 (regardless of `+` state).
        let is_next_label = !starts_indented
            && find_dlist_marker(trimmed_c).is_some()
            && !is_attribute_line(trimmed_c);
        let is_section_heading = !starts_indented
            && trimmed_c.starts_with('=')
            && trimmed_c.trim_start_matches('=').starts_with(' ');
        let is_block_attr = !starts_indented && is_attribute_line(trimmed_c);
        if is_next_label || is_section_heading || is_block_attr {
            break;
        }
        consecutive_blanks = 0;
        if is_plus {
            // `+` continuation: render as a paragraph break, and
            // arm the after-plus state so the next non-indented
            // block is still absorbed into this item.
            if !matches!(body_lines.last().map(String::as_str), Some("") | None) {
                body_lines.push(String::new());
            }
            after_plus = true;
            j += 1;
            continue;
        }
        if !starts_indented && !after_plus {
            // Plain top-level content with no `+` priming: end of this list item.
            break;
        }
        // Strip the leading indentation; keep the rest verbatim.
        let stripped = candidate
            .strip_prefix('\t')
            .or_else(|| candidate.strip_prefix("    "))
            .or_else(|| candidate.strip_prefix("   "))
            .or_else(|| candidate.strip_prefix("  "))
            .unwrap_or(candidate);
        body_lines.push(stripped.trim_end().to_string());
        // After-plus only protects ONE following block;
        // once non-indented content starts being consumed,
        // subsequent non-indented content needs another `+` to qualify.
        // The `+`-attached block counts as "consumed" once an empty-line boundary is hit inside it.
        // (The empty-line branch resets `after_plus = false`.)
        j += 1;
    }
    let consumed = j - start;
    // Trim trailing blank lines from the body: they belong to the boundary between items, not to this item.
    while body_lines.last().is_some_and(String::is_empty) {
        body_lines.pop();
    }
    if body_lines.is_empty() {
        // Bare `label::` with no body: keep just the label.
        return Some(DescriptionListRewrite {
            rewritten: format!("**{label_t}**\n"),
            consumed,
        });
    }
    let body = body_lines.join("\n");
    let rewritten = format!("**{label_t}**\n\n{body}\n");
    Some(DescriptionListRewrite {
        rewritten,
        consumed,
    })
}

/// Find the position and length of a description-list marker,
/// ignoring occurrences inside backtick-delimited literal spans (so `\`a::b\`` stays untouched).
/// AsciiDoc supports `::`, `:::`, and `::::` as progressively-nested dlist markers;
/// all of them collapse into the same `**label**\n\nbody` rewrite
/// (the hierarchy is lossy but the content survives, which is the priority).
/// Returns `None` when no marker is present or the line starts with the marker (a bare `::` is not a label).
fn find_dlist_marker(line: &str) -> Option<(usize, usize)> {
    let bytes = line.as_bytes();
    let mut in_backtick = false;
    let mut i = 0;
    while i + 1 < bytes.len() {
        let c = bytes[i];
        if c == b'`' {
            in_backtick = !in_backtick;
            i += 1;
            continue;
        }
        if !in_backtick && c == b':' && bytes[i + 1] == b':' {
            if i == 0 {
                return None;
            }
            // Detect run length: `::`, `:::`, or `::::`.
            let mut len = 2;
            while bytes.get(i + len) == Some(&b':') && len < 4 {
                len += 1;
            }
            return Some((i, len));
        }
        i += 1;
    }
    None
}

/// Detect AsciiDoc block attribute lines like `[source,rust]` or `[verse]`.
/// These look like description-list labels because of the `[..]` shape but should never be rewritten.
fn is_attribute_line(label: &str) -> bool {
    let t = label.trim();
    t.starts_with('[') && t.ends_with(']')
}

/// Strip residue acdc leaves behind:
/// - `<!-- Warning: ... -->` markers it emits whenever it falls back on a fallback rendering path,
///   (description lists, audio, callout lists, etc.).
///   These are pure noise to a downstream markdown reader.
/// - `linkgit:foo[N]` macros emitted as `linkgit:foo\[N\]`:
///   the surrounding bracket escapes are wrong markdown and the macro text isn't a real link target.
///   Rendered as plain `\`foo(N)\`` (manpage convention), dropping the prefix,
///   so the reading experience matches the AsciiDoc HTML output.
fn postprocess_markdown(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for line in raw.lines() {
        if line.trim_start().starts_with("<!-- Warning:") && line.trim_end().ends_with("-->") {
            // Drop standalone warning-comment lines outright;
            // a line combining a warning comment with content would need a finer strip,
            // but acdc only emits them on their own line.
            continue;
        }
        out.push_str(&strip_linkgit_macros(line));
        out.push('\n');
    }
    // Collapse the runs of blank lines that fall out when warning lines get dropped:
    // keeps output tidy and reduces diff noise,
    // when the markdown ends up rendered side-by-side with the original adoc.
    collapse_blank_lines(&out)
}

fn strip_linkgit_macros(line: &str) -> String {
    // Match `linkgit:NAME\[N\]` (escaped) or `linkgit:NAME[N]` (raw) and replace with backticked `NAME(N)`.
    // Done manually to avoid pulling in a regex dep just for this; the format is tightly bounded.
    let mut out = String::with_capacity(line.len());
    let bytes = line.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i..].starts_with(b"linkgit:") {
            let after_prefix = i + b"linkgit:".len();
            if let Some(rendered_len) = try_render_linkgit(&line[after_prefix..], &mut out) {
                i = after_prefix + rendered_len;
                continue;
            }
        }
        // Push one UTF-8 char at a time so we don't slice mid-codepoint.
        // NOTE: the enclosing `while i < bytes.len()` guard, plus `i`
        // only ever advancing by a prior char's `len_utf8()`, guarantees
        // `line[i..]` is a valid, non-empty UTF-8 slice here.
        #[allow(clippy::expect_used)]
        let ch = line[i..].chars().next().expect("non-empty slice");
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

fn try_render_linkgit(rest: &str, out: &mut String) -> Option<usize> {
    let bytes = rest.as_bytes();
    let mut name_end = 0;
    while name_end < bytes.len() {
        let c = bytes[name_end];
        if c.is_ascii_alphanumeric() || c == b'-' || c == b'_' {
            name_end += 1;
        } else {
            break;
        }
    }
    if name_end == 0 {
        return None;
    }
    let after_name = &bytes[name_end..];
    // Accept either `[N]` or `\[N\]` (the converter sometimes
    // escapes the brackets when it can't resolve the macro).
    let (volume, consumed_after) = if after_name.starts_with(b"\\[") {
        let payload = &rest[name_end + 2..];
        let close = payload.find("\\]")?;
        (&payload[..close], 2 + close + 2)
    } else if after_name.starts_with(b"[") {
        let payload = &rest[name_end + 1..];
        let close = payload.find(']')?;
        (&payload[..close], 1 + close + 1)
    } else {
        return None;
    };
    if volume.is_empty() || !volume.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let name = &rest[..name_end];
    out.push('`');
    out.push_str(name);
    out.push('(');
    out.push_str(volume);
    out.push_str(")`");
    Some(name_end + consumed_after)
}

fn collapse_blank_lines(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_blank = false;
    for line in s.lines() {
        let blank = line.trim().is_empty();
        if blank && prev_blank {
            continue;
        }
        out.push_str(line);
        out.push('\n');
        prev_blank = blank;
    }
    out
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn is_adoc_filename_accepts_both_extensions_case_insensitively() {
        assert!(is_adoc_filename("coding-rules.adoc"));
        assert!(is_adoc_filename("CODING-RULES.ADOC"));
        assert!(is_adoc_filename("coding-rules.asciidoc"));
        assert!(is_adoc_filename("CODING-RULES.AsciiDoc"));
    }

    #[test]
    fn is_adoc_filename_rejects_markdown_and_plain_names() {
        assert!(!is_adoc_filename("coding-rules.md"));
        assert!(!is_adoc_filename("coding-rules"));
        assert!(!is_adoc_filename(""));
        assert!(!is_adoc_filename(".adoc"));
        assert!(!is_adoc_filename("trailing."));
    }

    #[test]
    fn convert_preserves_heading_and_paragraph_text() {
        let source = "= Coding Rules\n\nA short introduction.\n";
        let markdown = convert_adoc_to_markdown(source).expect("convert");
        assert!(
            markdown.contains("Coding Rules"),
            "heading text must survive the conversion; got:\n{markdown}"
        );
        assert!(
            markdown.contains("A short introduction."),
            "paragraph text must survive the conversion; got:\n{markdown}"
        );
    }

    #[test]
    fn convert_emits_fenced_code_blocks() {
        let source = "= Example\n\n[source,rust]\n----\nfn main() {}\n----\n";
        let markdown = convert_adoc_to_markdown(source).expect("convert");
        assert!(
            markdown.contains("fn main() {}"),
            "code body must appear in the output; got:\n{markdown}"
        );
    }

    #[test]
    fn convert_surfaces_parse_errors_as_structured_variant() {
        let markdown = convert_adoc_to_markdown("").expect("empty input converts");
        assert!(markdown.is_empty() || markdown.trim().is_empty());
    }

    // ── Regression suite for the four production corruptions ────────

    #[test]
    fn convert_renders_description_list_label_and_body() {
        // Two-line form: `label::\n\tbody` is the dominant shape in
        // git's `config/protocol.adoc` and the `gitprotocol-*` files.
        // Before the preprocessor landed acdc dropped the body and
        // emitted only `<!-- Warning: description lists ... -->`.
        let source = "= Title\n\nprotocol.allow::\n\tDefault policy for unknown protocols.\n";
        let markdown = convert_adoc_to_markdown(source).expect("convert");
        assert!(
            markdown.contains("protocol.allow"),
            "label text must survive: {markdown}"
        );
        assert!(
            markdown.contains("Default policy for unknown protocols."),
            "body text must survive: {markdown}"
        );
        assert!(
            !markdown.contains("Warning: description lists"),
            "warning comment must be stripped: {markdown}"
        );
    }

    #[test]
    fn debug_preprocess_only_full_file_gitremote_helpers() {
        let path = "../../tmp/git/Documentation/gitremote-helpers.adoc";
        let Ok(source) = std::fs::read_to_string(path) else {
            eprintln!("skip: file {path} not present");
            return;
        };
        let pre = preprocess_adoc(&source);
        let labels = [
            "**'list'**",
            "**'list for-push'**",
            "**'option'",
            "**'fetch'",
            "**'push'",
            "**'import'**",
            "**'export'**",
            "**'connect'",
            "**'stateless-connect'",
            "**'get'",
            "**'unchanged'**",
            "**'object-format'**",
            "**'option verbosity'",
            "**'option progress'",
        ];
        let mut missing = Vec::new();
        for n in labels {
            if !pre.contains(n) {
                missing.push(n);
            }
        }
        eprintln!("preprocessed length: {}", pre.len());
        eprintln!("source length: {}", source.len());
        eprintln!("missing labels: {missing:?}");
        if !missing.is_empty() {
            // Print 1000 chars around the FIRST missing label's source location.
            for n in &missing[..missing.len().min(1)] {
                let label_src = format!("{}::", n.trim_start_matches("**").trim_end_matches("**"));
                if let Some(pos) = source.find(&label_src) {
                    let lo = pos.saturating_sub(200);
                    let hi = (pos + 800).min(source.len());
                    eprintln!("--- source around `{n}` ---\n{}\n---", &source[lo..hi]);
                }
            }
            panic!("preprocessor lost dlist labels");
        }
    }

    #[test]
    fn debug_full_file_gitremote_helpers_retains_dlist_bodies() {
        // Loads the actual upstream file, runs the full pipeline,
        // and checks for content from each major dlist section.
        // Helps catch global parser-state issues that aren't visible
        // in isolated excerpts.
        let path = "../../tmp/git/Documentation/gitremote-helpers.adoc";
        let Ok(source) = std::fs::read_to_string(path) else {
            eprintln!("skip: file {path} not present");
            return;
        };
        let markdown = convert_adoc_to_markdown(&source).expect("convert real file");
        let needles = [
            "Helper programs to interact with remote",
            "list of refs",
            "unchanged since the last import",
            "given hash algorithm",
            "Changes the verbosity",
            "Enables (or disables) progress",
        ];
        let mut missing = Vec::new();
        for n in needles {
            if !markdown.contains(n) {
                missing.push(n);
            }
        }
        if !missing.is_empty() {
            eprintln!("body length: {}", markdown.len());
            eprintln!("missing needles: {missing:?}");
            eprintln!(
                "body tail:\n{}",
                &markdown[markdown.len().saturating_sub(2000)..]
            );
            panic!("real-file conversion lost content");
        }
    }

    #[test]
    fn debug_preprocess_handles_emphasized_dlist_label() {
        // In `gitremote-helpers.adoc` the labels are wrapped in AsciiDoc single-quote emphasis,
        // e.g. `'unchanged'::` and `'option verbosity' <n>::`.
        // The preprocessor must rewrite both into bold paragraphs and absorb their bodies.
        let source = "= Title\n\n\
            REF LIST ATTRIBUTES\n\
            -------------------\n\n\
            Prelude paragraph.\n\n\
            'unchanged'::\n\
            \tThis ref is unchanged since the last import or fetch.\n\n\
            REF LIST KEYWORDS\n\
            -----------------\n\n\
            Prelude.\n\n\
            'object-format'::\n\
            \tRefs use the given hash algorithm.\n\n\
            OPTIONS\n\
            -------\n\n\
            'option verbosity' <n>::\n\
            \tChanges the verbosity of messages.\n";
        let preprocessed = preprocess_adoc(source);
        for needle in &[
            "**'unchanged'**",
            "unchanged since the last import",
            "**'object-format'**",
            "given hash algorithm",
            "**'option verbosity' <n>**",
            "Changes the verbosity",
        ] {
            assert!(
                preprocessed.contains(needle),
                "preprocessor lost `{needle}`:\n--- preprocessed ---\n{preprocessed}\n---"
            );
        }
        let markdown = convert_adoc_to_markdown(source).expect("convert");
        for needle in &[
            "unchanged since the last import",
            "given hash algorithm",
            "Changes the verbosity",
        ] {
            assert!(
                markdown.contains(needle),
                "converter dropped `{needle}`:\n--- markdown ---\n{markdown}\n---"
            );
        }
    }

    #[test]
    fn debug_preprocess_handles_real_gitrepository_layout_excerpt() {
        // Regression test: guards against the description-list body-loss pattern in real gitrepository-layout docs.
        // After preprocess+convert, the body MUST contain text from the `objects::`,
        // `objects/info::`, and `HEAD::` items.
        let source = "\
These things may exist in a Git repository.

objects::
\tObject store associated with this repository.  Usually
\tan object store is self sufficient (i.e. all the objects
\tthat are referred to by an object found in it are also
\tfound in it), but there are a few ways to violate it.
+
. You could have an incomplete but locally usable repository
by creating a shallow clone.  See linkgit:git-clone[1].
+
This directory is ignored if $GIT_COMMON_DIR is set.

objects/[0-9a-f][0-9a-f]::
\tA newly created object is stored in its own file.

objects/pack::
\tPacks (files that store many objects in compressed form,
\talong with index files to allow them to be randomly
\taccessed) are found in this directory.

HEAD::
\tA symref (see glossary) to the `refs/heads/` namespace
\tdescribing the currently active branch.
";
        let preprocessed = preprocess_adoc(source);
        // The preprocessor must rewrite each `LABEL::` into a bold
        // paragraph and absorb its body block.
        for needle in &[
            "**objects**",
            "Object store associated with this repository.",
            "shallow clone",
            "**objects/pack**",
            "Packs (files that store many objects",
            "**HEAD**",
            "currently active branch.",
        ] {
            assert!(
                preprocessed.contains(needle),
                "preprocessor lost `{needle}`:\n--- preprocessed ---\n{preprocessed}\n---"
            );
        }
        let markdown = convert_adoc_to_markdown(source).expect("convert");
        for needle in &[
            "objects",
            "Object store associated",
            "Packs (files that store many",
            "currently active branch.",
        ] {
            assert!(
                markdown.contains(needle),
                "converter dropped `{needle}`:\n--- markdown ---\n{markdown}\n---"
            );
        }
    }

    #[test]
    fn convert_absorbs_plus_attached_ordered_list_into_description_item() {
        // `gitrepository-layout.adoc`'s `objects::` entry uses a `+` continuation
        // followed by a `.`-prefixed ordered list *at column 0*.
        // AsciiDoc treats the ordered list as attached to the description item;
        // the preprocessor must absorb it instead of breaking the item early at the first non-indented line.
        let source = "= Title\n\nobjects::\n\
            \tBody one.\n\
            +\n\
            . First sub-item.\n\
            . Second sub-item.\n\
            +\n\
            \tBody two.\n\n\
            objects/info::\n\
            \tNext item body.\n";
        let markdown = convert_adoc_to_markdown(source).expect("convert");
        for needle in &[
            "Body one.",
            "First sub-item.",
            "Second sub-item.",
            "Body two.",
            "Next item body.",
        ] {
            assert!(
                markdown.contains(needle),
                "needle `{needle}` lost in conversion: {markdown}"
            );
        }
    }

    #[test]
    fn convert_keeps_full_multi_line_description_list_body() {
        // `gitrepository-layout.adoc` is the worst offender for this:
        // every entry in the repo-layout enumeration has 3-10 continuation lines plus `+`-bridged paragraphs.
        // The rewrite must absorb every continuation line (tab- or space-indented) plus `+` paragraph breaks,
        // so the body lands as a single multi-paragraph block.
        let source = "= Title\n\nobjects::\n\
            \tFirst paragraph line one.\n\
            \tFirst paragraph line two.\n\
            +\n\
            \tSecond paragraph after a `+` continuation.\n\n\
            objects/info::\n\
            \tNext item — must not absorb objects's body.\n";
        let markdown = convert_adoc_to_markdown(source).expect("convert");
        for needle in &[
            "First paragraph line one.",
            "First paragraph line two.",
            "Second paragraph after a",
            "Next item",
        ] {
            assert!(
                markdown.contains(needle),
                "description-list body line `{needle}` must survive: {markdown}"
            );
        }
        // The `+` continuation must NOT leak into the body verbatim.
        // A bare `+` on its own line in the output would re-render as a list item or paragraph divider.
        assert!(
            !markdown.contains("\n+\n"),
            "raw `+` continuation marker must be eaten: {markdown}"
        );
    }

    #[test]
    fn convert_renders_inline_description_list() {
        // Inline form: `label:: body` on the same line.
        // Common in git's CLI option docs.
        let source = "= Title\n\n--depth::\n\tOnly clone N most recent commits.\n";
        let markdown = convert_adoc_to_markdown(source).expect("convert");
        assert!(markdown.contains("--depth"), "label survives: {markdown}");
        assert!(
            markdown.contains("Only clone N most recent commits."),
            "body survives: {markdown}"
        );
    }

    #[test]
    fn convert_renders_setext_tilde_underline_as_heading() {
        // `~~~~~` underlines parse as setext level-2 headings only when the parser's `setext` feature is enabled.
        // With it off, the underline becomes either a paragraph fragment or a free-floating `---` after acdc gives up.
        let source = "= Title\n\nSection\n~~~~~~~\n\nBody.\n";
        let markdown = convert_adoc_to_markdown(source).expect("convert");
        assert!(
            markdown.contains("Section"),
            "tilde-underlined heading text must survive: {markdown}"
        );
        assert!(
            markdown.contains("Body."),
            "body following tilde heading must survive: {markdown}"
        );
        assert!(
            !markdown.contains("~~~~~~"),
            "underline characters must not leak through: {markdown}"
        );
    }

    #[test]
    fn convert_renders_caret_underline_as_subheading() {
        let source = "= Title\n\nSub\n^^^^\n\nBody.\n";
        let markdown = convert_adoc_to_markdown(source).expect("convert");
        assert!(
            markdown.contains("Sub"),
            "caret-underlined heading text must survive: {markdown}"
        );
        assert!(
            !markdown.contains("^^^^"),
            "underline characters must not leak through: {markdown}"
        );
    }

    #[test]
    fn convert_keeps_linkgit_macro_target_visible() {
        // `linkgit:git-config[1]` is everywhere in upstream Git docs.
        // Before postprocess it landed as `linkgit:git-config\[1\]`;
        // we render it as a backticked `git-config(1)` so the manpage
        // reference stays scannable.
        let source = "= Title\n\nSee linkgit:git-config[1] for details.\n";
        let markdown = convert_adoc_to_markdown(source).expect("convert");
        assert!(
            markdown.contains("`git-config(1)`"),
            "linkgit macro must render as `name(volume)`: {markdown}"
        );
        assert!(
            !markdown.contains("linkgit:"),
            "linkgit prefix must be stripped: {markdown}"
        );
    }

    #[test]
    fn convert_strips_warning_comments() {
        // Preprocessing prevents the description-list warning,
        // but other warning kinds (audio, callout list) still surface from acdc.
        // Standalone warning-comment lines are blanket-dropped,
        // so the reading experience stays clean even when new ones appear upstream.
        let source = "= Title\n\nNote: see audio::audio.mp3[]\n";
        let markdown = convert_adoc_to_markdown(source).expect("convert");
        assert!(
            !markdown.contains("<!-- Warning"),
            "standalone warning comments must be stripped: {markdown}"
        );
    }

    #[test]
    fn preprocess_skips_dlist_rewrite_inside_listing_block() {
        // Inside `----` listing blocks the literal `::` is content, not a list marker.
        // The rewriter must keep its hands off.
        let source = "= Title\n\n----\nlabel::\n\tbody\n----\n";
        let normalized = preprocess_adoc(source);
        assert!(
            normalized.contains("label::"),
            "verbatim listing must keep `::` literal: {normalized}"
        );
    }

    #[test]
    fn preprocess_skips_dlist_rewrite_for_attribute_lines() {
        // `[source,rust]` looks like a label to a naive `::` matcher
        // (it's not, since it has no `::`, but pre-emptively guard
        // against the related shape `[verse]::`).
        let source = "= Title\n\n[source,rust]\n----\nlet x = 1;\n----\n";
        let normalized = preprocess_adoc(source);
        assert!(
            normalized.contains("[source,rust]"),
            "attribute line must pass through unchanged: {normalized}"
        );
    }

    #[test]
    fn collapse_blank_lines_squeezes_runs() {
        let input = "a\n\n\n\nb\n";
        assert_eq!(collapse_blank_lines(input), "a\n\nb\n");
    }
}
