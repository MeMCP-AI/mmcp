//! Parser and renderer for memory files.
//!
//! A memory file is a Markdown document preceded by a frontmatter
//! block. Supported formats:
//!
//! - **TOML** with `+++` fences (mmcp default)
//! - **YAML** with `---` fences
//! - **JSON** with `---` fences
//! - **TOML** with `---` fences
//!
//! All parsing is handled by `gray_matter` with appropriate
//! delimiter and engine configuration. On write, the file is
//! rendered back in the **same format** it was parsed from.
//! Use `normalize()` to explicitly convert to TOML `+++`.

use gray_matter::Matter;
use gray_matter::engine::{JSON, TOML as GmTOML, YAML};
use thiserror::Error;

use crate::memory::MemoryFrontmatter;

/// Which frontmatter format was detected on parse.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrontmatterFormat {
    /// TOML with `+++` fences.
    TomlPlus,
    /// YAML with `---` fences.
    Yaml,
    /// JSON with `---` fences.
    Json,
    /// TOML with `---` fences.
    TomlDash,
}

/// Parsed memory file: frontmatter plus raw body text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryFile {
    /// Parsed frontmatter block.
    pub frontmatter: MemoryFrontmatter,

    /// Markdown body.
    pub body: String,

    /// The format detected on parse. Used by `to_string()` to
    /// render back in the same format.
    pub format: FrontmatterFormat,
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

    /// The frontmatter could not be rendered back.
    #[error("failed to render memory frontmatter: {0}")]
    Render(String),
}

impl From<toml::ser::Error> for MemoryParseError {
    fn from(e: toml::ser::Error) -> Self {
        MemoryParseError::Render(e.to_string())
    }
}

impl From<serde_json::Error> for MemoryParseError {
    fn from(e: serde_json::Error) -> Self {
        MemoryParseError::Render(e.to_string())
    }
}

impl From<serde_yaml::Error> for MemoryParseError {
    fn from(e: serde_yaml::Error) -> Self {
        MemoryParseError::Render(e.to_string())
    }
}

impl MemoryFile {
    /// Parse a memory file from raw text.
    ///
    /// Auto-detects the frontmatter format:
    /// - `+++` fences -> TOML via gray_matter with `+++` delimiter
    /// - `---` fences -> tries YAML, JSON (if `{`), then TOML
    pub fn parse(source: &str) -> Result<Self, MemoryParseError> {
        let input = source.strip_prefix('\u{feff}').unwrap_or(source);
        let trimmed = input.trim_start();

        if trimmed.starts_with("+++") {
            return Self::parse_with::<GmTOML>(input, "+++", FrontmatterFormat::TomlPlus);
        }
        if trimmed.starts_with("---") {
            // Peek after fence to detect JSON
            let after_fence = trimmed
                .strip_prefix("---")
                .and_then(|s| s.strip_prefix('\n').or(s.strip_prefix("\r\n")))
                .unwrap_or("");
            if after_fence.trim_start().starts_with('{') {
                if let Ok(file) = Self::parse_with::<JSON>(input, "---", FrontmatterFormat::Json) {
                    return Ok(file);
                }
            }
            // YAML is the default for --- fences
            if let Ok(file) = Self::parse_with::<YAML>(input, "---", FrontmatterFormat::Yaml) {
                return Ok(file);
            }
            // Fall back to TOML with --- fences
            return Self::parse_with::<GmTOML>(input, "---", FrontmatterFormat::TomlDash);
        }

        Err(MemoryParseError::NoFrontmatter)
    }

    /// Parse using gray_matter with a specific engine and delimiter.
    ///
    /// `Matter::parse::<T>` (0.3+) deserializes into the target type
    /// inline and surfaces both frontmatter-extraction errors and
    /// `serde` errors through the same `Result`. We still map
    /// `data = None` to an explicit `Deserialize` error because the
    /// type signature differs ("frontmatter absent" vs "malformed").
    fn parse_with<E: gray_matter::engine::Engine>(
        input: &str,
        delimiter: &str,
        fmt: FrontmatterFormat,
    ) -> Result<Self, MemoryParseError> {
        let mut matter = Matter::<E>::new();
        matter.delimiter = delimiter.to_string();
        let parsed = matter
            .parse::<MemoryFrontmatter>(input)
            .map_err(|e| MemoryParseError::Deserialize(format!("{fmt:?}: {e}")))?;
        let frontmatter = parsed.data.ok_or_else(|| {
            MemoryParseError::Deserialize(format!("{fmt:?}: no frontmatter data found"))
        })?;
        Ok(Self {
            frontmatter,
            body: parsed.content,
            format: fmt,
        })
    }

    /// Render back in the **same format** as parsed.
    pub fn to_string(&self) -> Result<String, MemoryParseError> {
        match self.format {
            FrontmatterFormat::TomlPlus => self.render("+++", RenderEngine::Toml),
            FrontmatterFormat::Yaml => self.render("---", RenderEngine::Yaml),
            FrontmatterFormat::Json => self.render("---", RenderEngine::Json),
            FrontmatterFormat::TomlDash => self.render("---", RenderEngine::Toml),
        }
    }

    /// Explicitly render as TOML `+++` regardless of original format.
    pub fn to_toml_string(&self) -> Result<String, MemoryParseError> {
        self.render("+++", RenderEngine::Toml)
    }

    /// Convert this file's format to TOML `+++` (in-place).
    pub fn normalize(&mut self) {
        self.format = FrontmatterFormat::TomlPlus;
    }

    fn render(&self, delimiter: &str, engine: RenderEngine) -> Result<String, MemoryParseError> {
        let front = match engine {
            RenderEngine::Toml => toml::to_string_pretty(&self.frontmatter)?,
            RenderEngine::Json => serde_json::to_string_pretty(&self.frontmatter)?,
            RenderEngine::Yaml => serde_yaml::to_string(&self.frontmatter)?,
        };
        let mut out = String::with_capacity(front.len() + self.body.len() + 16);
        out.push_str(delimiter);
        out.push('\n');
        out.push_str(&front);
        if !front.ends_with('\n') {
            out.push('\n');
        }
        out.push_str(delimiter);
        out.push('\n');
        out.push_str(&self.body);
        Ok(out)
    }
}

enum RenderEngine {
    Toml,
    Json,
    Yaml,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::MemoryKind;

    const TOML_SAMPLE: &str = "+++\nname = \"Rust Coding Rules\"\ndescription = \"Strict Rust coding conventions\"\nkind = \"rule\"\nmandatory = true\ntags = [\"rust\", \"style\"]\n+++\n# Rust Coding Rules\n\nBody text.\n";

    const YAML_SAMPLE: &str = "---\nname: YAML Memory\ndescription: A memory in YAML format\nkind: reference\ntags:\n  - yaml\n  - test\n---\n# YAML Body\n\nThis was written in YAML.\n";

    const JSON_SAMPLE: &str = "---\n{\"name\": \"JSON Memory\", \"description\": \"A memory in JSON\", \"kind\": \"scratch\"}\n---\n# JSON Body\n\nWritten in JSON.\n";

    #[test]
    fn parses_toml_frontmatter() {
        let file = MemoryFile::parse(TOML_SAMPLE).expect("parse toml");
        assert_eq!(file.frontmatter.name, "Rust Coding Rules");
        assert_eq!(file.frontmatter.kind, MemoryKind::Rule);
        assert_eq!(file.format, FrontmatterFormat::TomlPlus);
        assert!(file.body.contains("# Rust Coding Rules"));
    }

    #[test]
    fn parses_yaml_frontmatter() {
        let file = MemoryFile::parse(YAML_SAMPLE).expect("parse yaml");
        assert_eq!(file.frontmatter.name, "YAML Memory");
        assert_eq!(file.frontmatter.kind, MemoryKind::Reference);
        assert_eq!(file.format, FrontmatterFormat::Yaml);
        assert!(file.body.contains("# YAML Body"));
    }

    #[test]
    fn parses_json_frontmatter() {
        let file = MemoryFile::parse(JSON_SAMPLE).expect("parse json");
        assert_eq!(file.frontmatter.name, "JSON Memory");
        assert_eq!(file.frontmatter.kind, MemoryKind::Scratch);
        assert_eq!(file.format, FrontmatterFormat::Json);
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
    fn toml_round_trip_preserves_format() {
        let parsed = MemoryFile::parse(TOML_SAMPLE).expect("parse");
        let rendered = parsed.to_string().expect("render");
        assert!(rendered.starts_with("+++"), "should stay TOML +++");
        let reparsed = MemoryFile::parse(&rendered).expect("reparse");
        assert_eq!(parsed.frontmatter, reparsed.frontmatter);
        assert_eq!(reparsed.format, FrontmatterFormat::TomlPlus);
    }

    #[test]
    fn yaml_round_trip_preserves_format() {
        let parsed = MemoryFile::parse(YAML_SAMPLE).expect("parse yaml");
        let rendered = parsed.to_string().expect("render");
        assert!(rendered.starts_with("---"), "should stay YAML ---");
        let reparsed = MemoryFile::parse(&rendered).expect("reparse");
        assert_eq!(parsed.frontmatter.name, reparsed.frontmatter.name);
    }

    #[test]
    fn json_round_trip_preserves_format() {
        let parsed = MemoryFile::parse(JSON_SAMPLE).expect("parse json");
        let rendered = parsed.to_string().expect("render");
        assert!(rendered.starts_with("---"), "should stay JSON ---");
        assert!(rendered.contains("\"name\""), "should contain JSON");
    }

    #[test]
    fn normalize_converts_yaml_to_toml() {
        let mut parsed = MemoryFile::parse(YAML_SAMPLE).expect("parse yaml");
        assert_eq!(parsed.format, FrontmatterFormat::Yaml);
        parsed.normalize();
        assert_eq!(parsed.format, FrontmatterFormat::TomlPlus);
        let rendered = parsed.to_string().expect("render");
        assert!(rendered.starts_with("+++"), "normalized to TOML +++");
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

    #[test]
    fn frontmatter_parses_without_id() {
        let file = MemoryFile::parse(TOML_SAMPLE).expect("parse toml");
        assert_eq!(file.frontmatter.id, None);
        let rendered = file.to_string().expect("render");
        assert!(
            !rendered.contains("\nid ="),
            "absent id must not serialize an id field: {rendered}"
        );
    }

    #[test]
    fn frontmatter_id_round_trips_when_set() {
        use uuid::Uuid;

        let source = "+++\nid = \"0196e5bb-a000-7000-8000-000000000001\"\nname = \"With Id\"\ndescription = \"has uuid\"\nkind = \"rule\"\n+++\nbody\n";
        let parsed = MemoryFile::parse(source).expect("parse");
        let expected = Uuid::parse_str("0196e5bb-a000-7000-8000-000000000001").unwrap();
        assert_eq!(parsed.frontmatter.id, Some(expected));
        let rendered = parsed.to_string().expect("render");
        assert!(
            rendered.contains("id = \"0196e5bb-a000-7000-8000-000000000001\""),
            "rendered frontmatter missing id: {rendered}"
        );
        let reparsed = MemoryFile::parse(&rendered).expect("reparse");
        assert_eq!(reparsed.frontmatter.id, Some(expected));
    }
}
