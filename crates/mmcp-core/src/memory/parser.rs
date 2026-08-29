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
            if after_fence.trim_start().starts_with('{')
                && let Ok(file) = Self::parse_with::<JSON>(input, "---", FrontmatterFormat::Json)
            {
                return Ok(file);
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

/// Parse only the frontmatter block of a memory file.
///
/// `MemoryFile::parse` copies the body three times through
/// `gray_matter` (once into a field mmcp never reads, once
/// rebuilding it line by line, once converting that rebuild into an
/// owned `String`) even when a caller only wants a couple of
/// frontmatter fields. This entry point locates the closing fence
/// with a line scan and hands only the enclosed slice to the same
/// `gray_matter` engine `MemoryFile::parse` uses, so a well-formed
/// file costs only the frontmatter's size, never the body's; a file
/// missing its closing fence still costs one copy of the scanned
/// tail, a third of `gray_matter`'s three body copies, but not
/// frontmatter-bounded on that malformed path.
///
/// Delimiter detection and error semantics match `MemoryFile::parse`
/// exactly: the same fences are tried in the same order, and the
/// same [`MemoryParseError`] variant is returned for the same
/// malformed input.
pub fn parse_frontmatter(source: &str) -> Result<MemoryFrontmatter, MemoryParseError> {
    let input = source.strip_prefix('\u{feff}').unwrap_or(source);
    let trimmed = input.trim_start();

    if trimmed.starts_with("+++") {
        return parse_frontmatter_with::<GmTOML>(input, "+++", FrontmatterFormat::TomlPlus);
    }
    if trimmed.starts_with("---") {
        // Peek after fence to detect JSON, same as MemoryFile::parse.
        let after_fence = trimmed
            .strip_prefix("---")
            .and_then(|s| s.strip_prefix('\n').or(s.strip_prefix("\r\n")))
            .unwrap_or("");
        if after_fence.trim_start().starts_with('{')
            && let Ok(fm) = parse_frontmatter_with::<JSON>(input, "---", FrontmatterFormat::Json)
        {
            return Ok(fm);
        }
        // YAML is the default for --- fences.
        if let Ok(fm) = parse_frontmatter_with::<YAML>(input, "---", FrontmatterFormat::Yaml) {
            return Ok(fm);
        }
        // Fall back to TOML with --- fences.
        return parse_frontmatter_with::<GmTOML>(input, "---", FrontmatterFormat::TomlDash);
    }

    Err(MemoryParseError::NoFrontmatter)
}

/// Parse the frontmatter slice with a specific engine and delimiter,
/// mirroring `MemoryFile::parse_with` without ever building the body.
fn parse_frontmatter_with<E: gray_matter::engine::Engine>(
    input: &str,
    delimiter: &str,
    fmt: FrontmatterFormat,
) -> Result<MemoryFrontmatter, MemoryParseError> {
    let matter = extract_frontmatter_block(input, delimiter).ok_or_else(|| {
        MemoryParseError::Deserialize(format!("{fmt:?}: no frontmatter data found"))
    })?;
    let pod = <E as gray_matter::engine::Engine>::parse(&matter)
        .map_err(|e| MemoryParseError::Deserialize(format!("{fmt:?}: {e}")))?;
    pod.deserialize::<MemoryFrontmatter>()
        .map_err(|e| MemoryParseError::Deserialize(format!("{fmt:?}: {e}")))
}

/// Scan `input` for a closing fence and return the raw frontmatter
/// text found between the opening and closing `delimiter` lines.
///
/// Mirrors `gray_matter::Matter::parse`'s own fence detection line
/// for line (a line matches only when it equals `delimiter` once
/// trailing whitespace is trimmed) but never builds the post-fence
/// body: the scan returns the instant the closing fence is found, so
/// a well-formed file copies only the frontmatter block into `acc`.
/// A file with no closing fence still scans and accumulates every
/// remaining line before giving up - one copy of that tail, not
/// `gray_matter`'s three, and still nothing once a closing fence is
/// actually present. Returns `None` when the opening fence is
/// missing, no closing fence follows it, or the enclosed text is
/// blank once trimmed - the same three cases `gray_matter` leaves
/// its parsed data unset for.
fn extract_frontmatter_block(input: &str, delimiter: &str) -> Option<String> {
    if input.is_empty() || input.len() <= delimiter.len() {
        return None;
    }
    let (first_line, rest) = input.split_once('\n')?;
    if first_line.trim_end() != delimiter {
        return None;
    }
    let mut acc = String::new();
    for line in rest.lines() {
        if line.trim_end() == delimiter {
            let matter = acc.trim().to_string();
            return if matter.is_empty() {
                None
            } else {
                Some(matter)
            };
        }
        acc.push('\n');
        acc.push_str(line);
    }
    None
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
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

    /// A memory carrying `source = "<uuid>"` round-trips
    /// through parse + render without losing or coercing the field.
    /// Unset stays unset (the serializer skips); set survives.
    #[test]
    fn source_field_round_trips_through_toml() {
        use uuid::Uuid;
        let id: Uuid = "019d9d93-7fa9-7e91-acf8-61b16a8ee986".parse().unwrap();
        let with_source = format!(
            "+++\nname = \"src\"\ndescription = \"with source\"\nkind = \"rule\"\nsource = \"{id}\"\n+++\nbody\n"
        );
        let parsed = MemoryFile::parse(&with_source).expect("parse");
        assert_eq!(parsed.frontmatter.source, Some(id));
        let rendered = parsed.to_string().expect("render");
        let reparsed = MemoryFile::parse(&rendered).expect("reparse");
        assert_eq!(reparsed.frontmatter.source, Some(id));

        let without_source =
            "+++\nname = \"plain\"\ndescription = \"plain memory\"\nkind = \"rule\"\n+++\nbody\n";
        let parsed = MemoryFile::parse(without_source).expect("parse plain");
        assert_eq!(parsed.frontmatter.source, None);
        let rendered = parsed.to_string().expect("render plain");
        // The serializer skips the field when None: checking for
        // the literal `source =` key is the precise assertion (a
        // bare "source" substring would false-match prose in
        // `description`).
        assert!(
            !rendered.contains("source ="),
            "absent source must stay absent on render; got:\n{rendered}",
        );
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

    /// A memory whose body embeds a line that looks like the closing
    /// fence for the wrong delimiter (`---` inside a `+++`-fenced
    /// file's body is unrelated) or a value string that merely
    /// contains the delimiter substring without matching a whole
    /// trimmed line.
    const DELIMITER_LOOKALIKE_YAML: &str = "---\nname: \"troublesome --- value\"\ndescription: has a lookalike\nkind: reference\n---\nbody with a lone --- later\n\n---\n\nmore body\n";

    const EMPTY_BODY_TOML: &str =
        "+++\nname = \"empty body\"\ndescription = \"no trailing text\"\nkind = \"rule\"\n+++\n";

    const NO_CLOSING_FENCE: &str =
        "+++\nname = \"unterminated\"\ndescription = \"missing close\"\nkind = \"rule\"\n";

    const NO_FENCE_AT_ALL: &str = "Just plain text, no fences.\n";

    /// Assert `parse_frontmatter` returns the identical frontmatter
    /// `MemoryFile::parse` produces for every representative shape:
    /// each supported delimiter/engine, CRLF, a BOM prefix, an empty
    /// body, and a body that embeds delimiter-lookalike text.
    #[test]
    fn parse_frontmatter_matches_full_parse_for_valid_inputs() {
        let samples = [
            TOML_SAMPLE,
            YAML_SAMPLE,
            JSON_SAMPLE,
            DELIMITER_LOOKALIKE_YAML,
            EMPTY_BODY_TOML,
        ];
        for source in samples {
            let full = MemoryFile::parse(source).expect("full parse must succeed");
            let fast = parse_frontmatter(source).expect("frontmatter parse must succeed");
            assert_eq!(
                fast, full.frontmatter,
                "parse_frontmatter diverged from MemoryFile::parse for:\n{source}"
            );
        }
    }

    /// Same equivalence property for CRLF line endings and a
    /// UTF-8 BOM prefix, which shift byte offsets relative to the
    /// LF-only samples above.
    #[test]
    fn parse_frontmatter_matches_full_parse_for_crlf_and_bom() {
        let crlf = "+++\r\nname = \"crlf\"\r\ndescription = \"windows\"\r\nkind = \"rule\"\r\n+++\r\nbody\r\n";
        let bom = "\u{feff}+++\nname = \"bom\"\ndescription = \"bom prefixed\"\nkind = \"rule\"\n+++\nbody\n";
        for source in [crlf, bom] {
            let full = MemoryFile::parse(source).expect("full parse must succeed");
            let fast = parse_frontmatter(source).expect("frontmatter parse must succeed");
            assert_eq!(fast, full.frontmatter);
        }
    }

    /// Malformed input must return the same `MemoryParseError`
    /// variant, with the same message, as `MemoryFile::parse` -
    /// never a panic.
    #[test]
    fn parse_frontmatter_matches_full_parse_errors_for_malformed_input() {
        let malformed = [
            NO_FENCE_AT_ALL,
            NO_CLOSING_FENCE,
            "+++\nnot valid toml =====\n+++\nbody\n",
        ];
        for source in malformed {
            let full_err = MemoryFile::parse(source).expect_err("full parse must fail");
            let fast_err = parse_frontmatter(source).expect_err("frontmatter parse must fail");
            assert_eq!(
                std::mem::discriminant(&full_err),
                std::mem::discriminant(&fast_err),
                "error variant diverged for:\n{source}"
            );
            assert_eq!(
                full_err.to_string(),
                fast_err.to_string(),
                "error message diverged for:\n{source}"
            );
        }
    }

    /// `parse_frontmatter`'s cost scales with the frontmatter block,
    /// not the body: build a multi-megabyte body and confirm the
    /// frontmatter-only path runs meaningfully faster than the full
    /// parse, which must copy that body three times over. The margin
    /// is generous (an order of magnitude) so ordinary scheduling
    /// jitter cannot flip the comparison.
    #[test]
    fn parse_frontmatter_skips_the_body_copy_on_large_input() {
        const LARGE_BODY_LINES: usize = 200_000;
        const REQUIRED_SLOWDOWN_FACTOR: u32 = 3;

        let mut body = String::new();
        for i in 0..LARGE_BODY_LINES {
            body.push_str("line of repeated body content number ");
            body.push_str(&i.to_string());
            body.push('\n');
        }
        let source = format!(
            "+++\nname = \"large\"\ndescription = \"large body\"\nkind = \"rule\"\n+++\n{body}"
        );

        let fast_start = std::time::Instant::now();
        let fast = parse_frontmatter(&source).expect("frontmatter parse must succeed");
        let fast_elapsed = fast_start.elapsed();

        let full_start = std::time::Instant::now();
        let full = MemoryFile::parse(&source).expect("full parse must succeed");
        let full_elapsed = full_start.elapsed();

        assert_eq!(fast, full.frontmatter);
        assert!(
            fast_elapsed.saturating_mul(REQUIRED_SLOWDOWN_FACTOR) < full_elapsed,
            "expected parse_frontmatter ({fast_elapsed:?}) to be at least \
             {REQUIRED_SLOWDOWN_FACTOR}x faster than MemoryFile::parse ({full_elapsed:?}) \
             on a large body"
        );
    }

    /// A closing fence found but whose enclosed text is blank once
    /// trimmed carries no frontmatter data in `gray_matter`; both
    /// entry points must agree it is the same "no data" error.
    #[test]
    fn parse_frontmatter_matches_full_parse_for_blank_matter() {
        let source = "+++\n\n+++\nbody\n";
        let full_err = MemoryFile::parse(source).expect_err("full parse must fail");
        let fast_err = parse_frontmatter(source).expect_err("frontmatter parse must fail");
        assert_eq!(full_err.to_string(), fast_err.to_string());
    }
}
