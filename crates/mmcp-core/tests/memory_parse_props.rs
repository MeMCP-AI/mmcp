//! Property-based tests for `MemoryFile::parse` and its renderers.
//!
//! The parser feeds `gray_matter` plus three deserializers (TOML,
//! YAML, JSON) and is reachable from every MCP write path, so a
//! single panicking input would take out a whole session. These
//! properties prove:
//!
//! 1. `parse` never panics on arbitrary `&str`.
//! 2. Any `MemoryFile` that round-trips via `to_string()` → `parse`
//!    reconstructs an equivalent value — stable under the serde
//!    formats we support.
//! 3. `to_toml_string` always produces TOML-fenced output that
//!    re-parses to a TOML-formatted `MemoryFile`.

use mmcp_core::memory::{BumpIntent, FrontmatterFormat, MemoryFile, MemoryFrontmatter, MemoryKind};
use proptest::prelude::*;

// ── Generators ───────────────────────────────────────────────────────

/// Characters we let into frontmatter string fields. Excludes the
/// fence sequences and quoting delimiters that break TOML/YAML/JSON
/// round trips. The goal of this property suite is to catch parser
/// *bugs*, not to re-test the underlying serializers.
fn safe_text_strategy() -> impl Strategy<Value = String> {
    prop::string::string_regex("[a-zA-Z0-9 _\\-]{1,48}").unwrap()
}

fn memory_kind_strategy() -> impl Strategy<Value = MemoryKind> {
    prop_oneof![
        Just(MemoryKind::Rule),
        Just(MemoryKind::Snapshot),
        Just(MemoryKind::Log),
        Just(MemoryKind::Reference),
        Just(MemoryKind::Scratch),
    ]
}

fn bump_intent_strategy() -> impl Strategy<Value = Option<BumpIntent>> {
    prop_oneof![
        Just(None),
        Just(Some(BumpIntent::Patch)),
        Just(Some(BumpIntent::Minor)),
        Just(Some(BumpIntent::Major)),
    ]
}

fn semver_strategy() -> impl Strategy<Value = Option<semver::Version>> {
    prop_oneof![
        Just(None),
        (0u64..100, 0u64..100, 0u64..100)
            .prop_map(|(ma, mi, pa)| Some(semver::Version::new(ma, mi, pa))),
    ]
}

fn tags_strategy() -> impl Strategy<Value = Vec<String>> {
    prop::collection::vec(
        prop::string::string_regex("[a-z0-9][a-z0-9\\-]{0,20}").unwrap(),
        0..5,
    )
}

fn frontmatter_strategy() -> impl Strategy<Value = MemoryFrontmatter> {
    (
        safe_text_strategy(),
        safe_text_strategy(),
        memory_kind_strategy(),
        any::<bool>(),
        semver_strategy(),
        tags_strategy(),
        bump_intent_strategy(),
    )
        .prop_map(
            |(name, description, kind, mandatory, version, tags, bump_intent)| {
                MemoryFrontmatter::new(name, description, kind)
                    .with_mandatory(mandatory)
                    .with_version(version)
                    .with_tags(tags)
                    .with_bump_intent(bump_intent)
            },
        )
}

/// Markdown-ish body that does not contain `+++` or `---` fence
/// lines — otherwise `gray_matter` could re-interpret the body as a
/// second frontmatter block and the round trip no longer holds.
///
/// The body is forced to start with a non-whitespace character so
/// the render→parse round trip does not collide with `gray_matter`'s
/// leading-newline consumption after the closing fence: renderers
/// emit `<fence>\n<body>`, and the parser drops a single leading
/// newline from what it hands back as the body, which is fine for
/// normal content but breaks the pointwise equality when the body
/// itself started with a newline.
fn body_strategy() -> impl Strategy<Value = String> {
    prop::string::string_regex("[a-zA-Z0-9]([a-zA-Z0-9 .,!?_\\-]{0,40}\\n){0,8}").unwrap()
}

fn format_strategy() -> impl Strategy<Value = FrontmatterFormat> {
    prop_oneof![
        Just(FrontmatterFormat::TomlPlus),
        Just(FrontmatterFormat::Yaml),
        Just(FrontmatterFormat::Json),
        Just(FrontmatterFormat::TomlDash),
    ]
}

fn memory_file_strategy() -> impl Strategy<Value = MemoryFile> {
    (frontmatter_strategy(), body_strategy(), format_strategy()).prop_map(
        |(frontmatter, body, format)| MemoryFile {
            frontmatter,
            body,
            format,
        },
    )
}

// ── Properties ───────────────────────────────────────────────────────

proptest! {
    // Small case budget by default; tweak with `PROPTEST_CASES` when
    // hunting regressions.
    #![proptest_config(ProptestConfig {
        cases: 64,
        .. ProptestConfig::default()
    })]

    /// `parse` on *any* byte sequence (treated as UTF-8) must return
    /// a `Result` rather than panic. Parser failure is the intended
    /// outcome for garbage; a panic would take a session out.
    #[test]
    fn parse_never_panics_on_arbitrary_text(
        raw in prop::string::string_regex(".{0,512}").unwrap()
    ) {
        let _ = MemoryFile::parse(&raw);
    }

    /// Valid `MemoryFile` values survive a render → parse round trip.
    /// The format is preserved (render wrote it, parse re-detected it)
    /// and the frontmatter + body round trip byte-for-byte modulo the
    /// renderer's normalization (trailing newline handling).
    #[test]
    fn render_parse_round_trip_preserves_file(
        original in memory_file_strategy()
    ) {
        let rendered = original.to_string().expect("render should succeed for valid frontmatter");
        let parsed = MemoryFile::parse(&rendered).expect("parse should succeed on our own output");
        prop_assert_eq!(parsed.frontmatter, original.frontmatter);
        prop_assert_eq!(parsed.format, original.format);
        // Body survives verbatim except for potential trailing
        // whitespace fixups a serializer might introduce; assert the
        // trimmed content round-trips.
        prop_assert_eq!(parsed.body.trim_end(), original.body.trim_end());
    }

    /// `to_toml_string` always emits `+++` fences and the output
    /// re-parses as `FrontmatterFormat::TomlPlus`.
    #[test]
    fn to_toml_string_emits_toml_plus_fences(
        file in memory_file_strategy()
    ) {
        let toml_rendered = file.to_toml_string().expect("toml render");
        prop_assert!(toml_rendered.starts_with("+++\n"));
        let parsed = MemoryFile::parse(&toml_rendered).expect("toml parse");
        prop_assert_eq!(parsed.format, FrontmatterFormat::TomlPlus);
        prop_assert_eq!(parsed.frontmatter, file.frontmatter);
    }

    /// `normalize()` mutates only the format; frontmatter and body
    /// are untouched, and the subsequent render/parse pair yields a
    /// TOML-fenced file.
    #[test]
    fn normalize_changes_only_the_format(
        file in memory_file_strategy()
    ) {
        let mut normalized = file.clone();
        normalized.normalize();
        prop_assert_eq!(normalized.format, FrontmatterFormat::TomlPlus);
        prop_assert_eq!(&normalized.frontmatter, &file.frontmatter);
        prop_assert_eq!(&normalized.body, &file.body);
    }
}
