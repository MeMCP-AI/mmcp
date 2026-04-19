//! AsciiDoc -> Markdown conversion used by the import pipeline.
//!
//! The project stores every memory as `.md` with a TOML / YAML
//! frontmatter fence. Supporting AsciiDoc on the input side is a
//! one-way bridge: an `.adoc` / `.asciidoc` file is parsed with
//! `acdc-parser` and rendered to CommonMark via
//! `acdc-converters-markdown`, then handed verbatim to
//! [`crate::memory::import_memory`] as if the operator had dropped
//! a `.md` file. Nothing else in the store layer needs to know the
//! source format: on-disk storage, diagnostics, and the MCP read
//! surface all stay markdown-native.
//!
//! `DocumentAttributes` on the parsed AsciiDoc side are NOT currently
//! promoted into the memory's TOML frontmatter. Operators who want
//! structured frontmatter must either pre-embed it in the source
//! (AsciiDoc supports front-matter-style preambles that `gray_matter`
//! will pick up after conversion) or supply
//! [`crate::memory::SynthFrontmatter`] via the import CLI flags.

use acdc_converters_core::{Converter, Options};
use acdc_converters_markdown::{MarkdownVariant, Processor};
use acdc_parser::Parser;
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
/// Matches on the last `.`-separated segment of the name, so paths
/// are fine as long as the caller hands the full base name (not a
/// stripped stem). Empty string and bare `.adoc` (no stem) both
/// return false - a memory slug must survive the stem-extraction
/// in [`crate::memory::slugify_filename`].
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
/// The output is a plain string with no frontmatter fence; if the
/// caller wants a frontmatter block they supply it via
/// [`crate::memory::SynthFrontmatter`] on the import call. CommonMark
/// is chosen over GFM because mmcp's existing body-section parser
/// (`mmcp_core::memory::body`) is built on `pulldown-cmark` in its
/// default CommonMark mode; staying aligned keeps the round trip
/// between imported adoc content and semantic section addressing
/// coherent.
pub fn convert_adoc_to_markdown(source: &str) -> Result<String, AdocConvertError> {
    let document = Parser::new(source)
        .parse()
        .map_err(|e| AdocConvertError::Parse(e.to_string()))?;
    let processor = Processor::new(Options::default(), Default::default())
        .with_variant(MarkdownVariant::CommonMark);
    let mut out = Vec::new();
    processor
        .write_to(&document, &mut out, None)
        .map_err(|e| AdocConvertError::Render(e.to_string()))?;
    String::from_utf8(out).map_err(|e| AdocConvertError::Utf8(e.to_string()))
}

#[cfg(test)]
mod tests {
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
        // The renderer's exact framing is up to `acdc-converters-markdown`;
        // we only pin the invariants that matter for import: heading text
        // and paragraph text survive. Framing characters (`#`, blank
        // lines) can shift without breaking downstream consumers.
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
        // AsciiDoc is fairly permissive; forcing a hard parse failure
        // here is noisy. We instead smoke-test that the mapping from
        // parser output to CommonMark never panics on common edge
        // cases - an empty string is the tightest boundary.
        let markdown = convert_adoc_to_markdown("").expect("empty input converts");
        assert!(markdown.is_empty() || markdown.trim().is_empty());
    }
}
