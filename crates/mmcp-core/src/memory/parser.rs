//! Parser and renderer for memory files.
//!
//! A memory file is a Markdown document preceded by a frontmatter
//! block. Supported formats:
//!
//! - **TOML** with `+++` fences (mmcp canonical format, hand-parsed)
//! - **YAML** with `---` fences (via `gray_matter`)
//! - **JSON** with `---` fences (via `gray_matter`)
//! - **TOML** with `---` fences (via `gray_matter`)
//!
//! On read, the parser auto-detects the format from the opening
//! fence. On write, the canonical `+++` TOML format is always
//! produced so all committed files have a consistent shape.

use gray_matter::Matter;
use gray_matter::engine::{JSON, TOML as GmTOML, YAML};
use thiserror::Error;

use crate::memory::MemoryFrontmatter;

/// Parsed memory file: frontmatter plus raw body text.
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
    /// No frontmatter block was detected in the file.
    #[error("no frontmatter detected (expected `+++` or `---` fences)")]
    NoFrontmatter,

    /// The frontmatter was found but could not be deserialized.
    #[error("invalid memory frontmatter: {0}")]
    Deserialize(String),

    /// The frontmatter could not be rendered back to TOML.
    #[error("failed to render memory frontmatter: {0}")]
    Render(#[from] toml::ser::Error),
}

impl From<toml::de::Error> for MemoryParseError {
    fn from(e: toml::de::Error) -> Self {
        MemoryParseError::Deserialize(e.to_string())
    }
}

/// Fence that opens and closes a TOML frontmatter block.
const TOML_FENCE: &str = "+++";

impl MemoryFile {
    /// Parse a memory file from raw text.
    ///
    /// Auto-detects the frontmatter format:
    /// - `+++` fences -> native TOML parser
    /// - `---` fences -> gray_matter (tries YAML, then TOML)
    ///
    /// Accepts UTF-8 BOM prefix and both LF and CRLF line endings.
    pub fn parse(source: &str) -> Result<Self, MemoryParseError> {
        let input = source.strip_prefix('\u{feff}').unwrap_or(source);
        let trimmed = input.trim_start();

        if trimmed.starts_with(TOML_FENCE) {
            return Self::parse_toml_native(input);
        }
        if trimmed.starts_with("---") {
            // Try YAML first (most common with --- fences)
            if let Ok(file) = Self::parse_gray_matter_yaml(input) {
                return Ok(file);
            }
            // Try JSON (--- fenced JSON block)
            if let Ok(file) = Self::parse_gray_matter_json(input) {
                return Ok(file);
            }
            // Fall back to TOML with --- fences
            return Self::parse_gray_matter_toml(input);
        }

        Err(MemoryParseError::NoFrontmatter)
    }

    /// Native TOML parser for `+++` fenced files.
    fn parse_toml_native(input: &str) -> Result<Self, MemoryParseError> {
        let after_open = strip_fence_line(input, TOML_FENCE)
            .ok_or(MemoryParseError::NoFrontmatter)?;
        let (frontmatter_text, body) = split_at_closing_fence(after_open, TOML_FENCE)
            .ok_or(MemoryParseError::Deserialize(
                "missing closing +++ fence".to_string(),
            ))?;
        let frontmatter: MemoryFrontmatter = toml::from_str(frontmatter_text)?;
        Ok(Self {
            frontmatter,
            body: body.to_string(),
        })
    }

    /// gray_matter YAML parser for `---` fenced files.
    fn parse_gray_matter_yaml(input: &str) -> Result<Self, MemoryParseError> {
        let matter = Matter::<YAML>::new();
        let result = matter
            .parse_with_struct::<MemoryFrontmatter>(input)
            .ok_or_else(|| {
                MemoryParseError::Deserialize("YAML frontmatter parse failed".to_string())
            })?;
        Ok(Self {
            frontmatter: result.data,
            body: result.content,
        })
    }

    /// gray_matter JSON parser for `---` fenced JSON files.
    fn parse_gray_matter_json(input: &str) -> Result<Self, MemoryParseError> {
        let matter = Matter::<JSON>::new();
        let result = matter
            .parse_with_struct::<MemoryFrontmatter>(input)
            .ok_or_else(|| {
                MemoryParseError::Deserialize("JSON frontmatter parse failed".to_string())
            })?;
        Ok(Self {
            frontmatter: result.data,
            body: result.content,
        })
    }

    /// gray_matter TOML parser for `---` fenced TOML files.
    fn parse_gray_matter_toml(input: &str) -> Result<Self, MemoryParseError> {
        let matter = Matter::<GmTOML>::new();
        let result = matter
            .parse_with_struct::<MemoryFrontmatter>(input)
            .ok_or_else(|| {
                MemoryParseError::Deserialize("TOML (---) frontmatter parse failed".to_string())
            })?;
        Ok(Self {
            frontmatter: result.data,
            body: result.content,
        })
    }

    /// Render the file back to the canonical `+++` TOML format.
    pub fn to_string(&self) -> Result<String, MemoryParseError> {
        let front = toml::to_string_pretty(&self.frontmatter)?;
        let mut out = String::with_capacity(front.len() + self.body.len() + 16);
        out.push_str(TOML_FENCE);
        out.push('\n');
        out.push_str(&front);
        if !front.ends_with('\n') {
            out.push('\n');
        }
        out.push_str(TOML_FENCE);
        out.push('\n');
        out.push_str(&self.body);
        Ok(out)
    }
}

/// Strip the opening fence line, returning everything after it.
fn strip_fence_line<'a>(source: &'a str, fence: &str) -> Option<&'a str> {
    let rest = source.strip_prefix(fence)?;
    let rest = rest.strip_prefix("\r\n").or_else(|| rest.strip_prefix('\n'))?;
    Some(rest)
}

/// Split at the first standalone fence line.
fn split_at_closing_fence<'a>(source: &'a str, fence: &str) -> Option<(&'a str, &'a str)> {
    let mut cursor = 0usize;
    while cursor < source.len() {
        let remaining = &source[cursor..];
        let line_end = remaining.find('\n').unwrap_or(remaining.len());
        let line = &remaining[..line_end];
        let trimmed = line.strip_suffix('\r').unwrap_or(line);

        if trimmed == fence {
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

    const TOML_SAMPLE: &str = "+++\nname = \"Rust Coding Rules\"\ndescription = \"Strict Rust coding conventions\"\nkind = \"rule\"\nmandatory = true\ntags = [\"rust\", \"style\"]\n+++\n# Rust Coding Rules\n\nBody text.\n";

    const YAML_SAMPLE: &str = "---\nname: YAML Memory\ndescription: A memory in YAML format\nkind: reference\ntags:\n  - yaml\n  - test\n---\n# YAML Body\n\nThis was written in YAML.\n";

    #[test]
    fn parses_toml_frontmatter() {
        let file = MemoryFile::parse(TOML_SAMPLE).expect("parse toml");
        assert_eq!(file.frontmatter.name, "Rust Coding Rules");
        assert_eq!(file.frontmatter.kind, MemoryKind::Rule);
        assert!(file.frontmatter.mandatory);
        assert_eq!(file.frontmatter.tags, vec!["rust", "style"]);
        assert!(file.body.starts_with("# Rust Coding Rules"));
    }

    #[test]
    fn parses_yaml_frontmatter() {
        let file = MemoryFile::parse(YAML_SAMPLE).expect("parse yaml");
        assert_eq!(file.frontmatter.name, "YAML Memory");
        assert_eq!(file.frontmatter.kind, MemoryKind::Reference);
        assert_eq!(file.frontmatter.tags, vec!["yaml", "test"]);
        assert!(file.body.contains("# YAML Body"));
    }

    const JSON_SAMPLE: &str = "---\n{\"name\": \"JSON Memory\", \"description\": \"A memory in JSON\", \"kind\": \"scratch\"}\n---\n# JSON Body\n\nWritten in JSON.\n";

    #[test]
    fn parses_json_frontmatter() {
        let file = MemoryFile::parse(JSON_SAMPLE).expect("parse json");
        assert_eq!(file.frontmatter.name, "JSON Memory");
        assert_eq!(file.frontmatter.kind, MemoryKind::Scratch);
        assert!(file.body.contains("# JSON Body"));
    }

    #[test]
    fn rejects_file_without_frontmatter() {
        let source = "Just plain text, no fences.\n";
        let err = MemoryFile::parse(source).expect_err("must fail");
        assert!(matches!(err, MemoryParseError::NoFrontmatter));
    }

    #[test]
    fn rejects_invalid_toml_in_frontmatter() {
        let source = "+++\nnot valid toml =====\n+++\nbody\n";
        let err = MemoryFile::parse(source).expect_err("must fail");
        assert!(matches!(err, MemoryParseError::Deserialize(_)));
    }

    #[test]
    fn round_trip_toml_preserves_content() {
        let parsed = MemoryFile::parse(TOML_SAMPLE).expect("parse");
        let rendered = parsed.to_string().expect("render");
        let reparsed = MemoryFile::parse(&rendered).expect("reparse");
        assert_eq!(parsed.frontmatter, reparsed.frontmatter);
    }

    #[test]
    fn yaml_round_trips_through_toml_render() {
        let parsed = MemoryFile::parse(YAML_SAMPLE).expect("parse yaml");
        let rendered = parsed.to_string().expect("render as toml");
        assert!(rendered.starts_with("+++"));
        let reparsed = MemoryFile::parse(&rendered).expect("reparse toml");
        assert_eq!(parsed.frontmatter.name, reparsed.frontmatter.name);
        assert_eq!(parsed.frontmatter.kind, reparsed.frontmatter.kind);
    }

    #[test]
    fn accepts_crlf_line_endings() {
        let source = "+++\r\nname = \"crlf\"\r\ndescription = \"windows\"\r\nkind = \"rule\"\r\n+++\r\nbody\r\n";
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
