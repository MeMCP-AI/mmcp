//! Parser and renderer for memory files.
//!
//! A memory file is a Markdown document preceded by a TOML
//! frontmatter block delimited by `+++` fences:
//!
//! ```markdown
//! +++
//! name = "Example"
//! description = "Demonstration memory"
//! kind = "rule"
//! +++
//!
//! # Body
//! Markdown content.
//! ```
//!
//! The parser is intentionally minimal and hand-rolled so that the
//! frontmatter schema stays tied to our own types without depending
//! on a YAML-first library.

use thiserror::Error;

use crate::memory::MemoryFrontmatter;

/// Fence that opens and closes the TOML frontmatter block.
const FENCE: &str = "+++";

/// Parsed memory file: frontmatter plus raw body text.
///
/// The body is exactly what followed the closing fence, with the
/// single newline that terminates the fence consumed. Trailing
/// whitespace is preserved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryFile {
    /// Parsed frontmatter block.
    pub frontmatter: MemoryFrontmatter,

    /// Markdown body.
    pub body: String,
}

/// Failures that can occur while parsing a memory file.
#[derive(Debug, Error)]
pub enum MemoryParseError {
    /// The file did not start with the opening `+++` fence.
    #[error("memory file must start with a `+++` frontmatter fence")]
    MissingOpeningFence,

    /// The opening fence was found but the closing fence was not.
    #[error("memory file is missing the closing `+++` frontmatter fence")]
    MissingClosingFence,

    /// The TOML inside the fences failed to parse against the
    /// [`MemoryFrontmatter`] schema.
    #[error("invalid memory frontmatter: {0}")]
    Toml(#[from] toml::de::Error),

    /// The frontmatter could not be rendered back to TOML.
    #[error("failed to render memory frontmatter: {0}")]
    Render(#[from] toml::ser::Error),
}

impl MemoryFile {
    /// Parse a memory file from raw text.
    ///
    /// Accepts both `\n` and `\r\n` line endings. The opening fence
    /// must be the very first line, and the closing fence must appear
    /// on its own line.
    pub fn parse(source: &str) -> Result<Self, MemoryParseError> {
        let without_bom = source.strip_prefix('\u{feff}').unwrap_or(source);

        let after_open = strip_fence_line(without_bom)
            .ok_or(MemoryParseError::MissingOpeningFence)?;

        let (frontmatter_text, body) = split_at_closing_fence(after_open)
            .ok_or(MemoryParseError::MissingClosingFence)?;

        let frontmatter: MemoryFrontmatter = toml::from_str(frontmatter_text)?;

        Ok(Self {
            frontmatter,
            body: body.to_string(),
        })
    }

    /// Render the file back to the `+++`-fenced format.
    pub fn to_string(&self) -> Result<String, MemoryParseError> {
        let front = toml::to_string_pretty(&self.frontmatter)?;
        let mut out = String::with_capacity(front.len() + self.body.len() + 16);
        out.push_str(FENCE);
        out.push('\n');
        out.push_str(&front);
        if !front.ends_with('\n') {
            out.push('\n');
        }
        out.push_str(FENCE);
        out.push('\n');
        out.push_str(&self.body);
        Ok(out)
    }
}

/// Strip the opening fence line from `source`, returning everything
/// after it or `None` if the first line was not a fence.
fn strip_fence_line(source: &str) -> Option<&str> {
    let rest = source.strip_prefix(FENCE)?;
    let rest = rest.strip_prefix("\r\n").or_else(|| rest.strip_prefix('\n'))?;
    Some(rest)
}

/// Split `source` at the first standalone `+++` line, returning the
/// text before the fence and the text after it (with the line break
/// following the fence consumed). Returns `None` if no closing fence
/// exists.
fn split_at_closing_fence(source: &str) -> Option<(&str, &str)> {
    let mut cursor = 0usize;
    while cursor < source.len() {
        let remaining = &source[cursor..];
        let line_end = remaining.find('\n').unwrap_or(remaining.len());
        let line = &remaining[..line_end];
        let trimmed = line.strip_suffix('\r').unwrap_or(line);

        if trimmed == FENCE {
            let front = &source[..cursor];
            let body_start = cursor + line_end + usize::from(line_end < remaining.len());
            let body = if body_start > source.len() {
                ""
            } else {
                &source[body_start..]
            };
            return Some((front, body));
        }

        cursor += line_end + usize::from(line_end < remaining.len());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::MemoryKind;

    const SAMPLE: &str = "+++\nname = \"Rust Coding Rules\"\ndescription = \"Strict Rust coding conventions\"\nkind = \"rule\"\nmandatory = true\ntags = [\"rust\", \"style\"]\n+++\n# Rust Coding Rules\n\nBody text.\n";

    #[test]
    fn parses_fenced_toml_frontmatter() {
        let file = MemoryFile::parse(SAMPLE).expect("parse sample");
        assert_eq!(file.frontmatter.name, "Rust Coding Rules");
        assert_eq!(file.frontmatter.kind, MemoryKind::Rule);
        assert!(file.frontmatter.mandatory);
        assert_eq!(file.frontmatter.tags, vec!["rust", "style"]);
        assert!(file.body.starts_with("# Rust Coding Rules"));
    }

    #[test]
    fn rejects_file_without_opening_fence() {
        let source = "name = \"no fence\"\n";
        let err = MemoryFile::parse(source).expect_err("must fail");
        assert!(matches!(err, MemoryParseError::MissingOpeningFence));
    }

    #[test]
    fn rejects_file_without_closing_fence() {
        let source = "+++\nname = \"open forever\"\n";
        let err = MemoryFile::parse(source).expect_err("must fail");
        assert!(matches!(err, MemoryParseError::MissingClosingFence));
    }

    #[test]
    fn rejects_invalid_toml_in_frontmatter() {
        let source = "+++\nnot valid toml =====\n+++\nbody\n";
        let err = MemoryFile::parse(source).expect_err("must fail");
        assert!(matches!(err, MemoryParseError::Toml(_)));
    }

    #[test]
    fn round_trip_preserves_frontmatter_and_body() {
        let parsed = MemoryFile::parse(SAMPLE).expect("parse sample");
        let rendered = parsed.to_string().expect("render");
        let reparsed = MemoryFile::parse(&rendered).expect("reparse");
        assert_eq!(parsed, reparsed);
    }

    #[test]
    fn accepts_crlf_line_endings() {
        let source = "+++\r\nname = \"crlf\"\r\ndescription = \"windows newlines\"\r\nkind = \"rule\"\r\n+++\r\nbody\r\n";
        let file = MemoryFile::parse(source).expect("parse crlf");
        assert_eq!(file.frontmatter.name, "crlf");
    }

    #[test]
    fn strips_utf8_bom() {
        let source = "\u{feff}+++\nname = \"bom\"\ndescription = \"bom prefixed\"\nkind = \"rule\"\n+++\nbody\n";
        let file = MemoryFile::parse(source).expect("parse bom");
        assert_eq!(file.frontmatter.name, "bom");
    }
}
