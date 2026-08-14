//! Property-based tests that the TOML parsers used by mmcp-core
//! never panic on arbitrary input.
//!
//! Each of these is the single entry point a user-edited config or
//! manifest passes through:
//!
//! - [`ProjectConfig::from_toml`] reads `.mmcp.toml` at the project
//!   root: every CLI invocation hits it.
//! - [`UserConfig::from_toml`] reads `~/.mmcp/config.toml`: every
//!   MCP session hits it during author resolution.
//! - [`GroupManifest::from_toml`] reads the `.mmcp.toml` committed
//!   inside each group repo: every `group_info`/`list_memories`
//!   tool call hits it.
//!
//! A panic on any of these means a malformed file can take the
//! whole client down. The proptests below generate bounded-length
//! text (both random bytes and plausible-looking TOML-ish fragments)
//! and assert each parser returns a `Result`, never panics.

use mmcp_core::config::{ProjectConfig, UserConfig};
use mmcp_core::manifest::GroupManifest;
use proptest::prelude::*;

/// Strategy producing short UTF-8 strings of arbitrary Unicode
/// characters. Captures the "random bytes" half of the adversarial
/// inputs we care about.
fn arbitrary_utf8_strategy() -> impl Strategy<Value = String> {
    prop::string::string_regex(".{0,256}").unwrap()
}

/// Strategy producing strings that resemble TOML (headers, keys,
/// values, arrays) but without being constrained to valid syntax.
/// Catches the "plausible-looking but subtly broken" inputs that
/// pure byte-level fuzzing tends to miss.
fn tomlish_strategy() -> impl Strategy<Value = String> {
    // Pre-generated tokens; we splice them into a random sequence.
    let tokens = prop::sample::select(vec![
        "[section]".to_string(),
        "key = \"value\"\n".to_string(),
        "num = 42\n".to_string(),
        "bool = true\n".to_string(),
        "list = [1, 2, 3]\n".to_string(),
        "nested = { a = \"b\" }\n".to_string(),
        "\"quoted key\" = \"value\"\n".to_string(),
        "empty = \"\"\n".to_string(),
        "project_uuid = \"00000000-0000-0000-0000-000000000000\"\n".to_string(),
        "schema_version = 1\n".to_string(),
        "mandatory = false\n".to_string(),
        "".to_string(),
    ]);
    prop::collection::vec(tokens, 0..12).prop_map(|v| v.join(""))
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 64,
        .. ProptestConfig::default()
    })]

    #[test]
    fn project_config_parser_never_panics_on_arbitrary_utf8(
        raw in arbitrary_utf8_strategy()
    ) {
        let _ = ProjectConfig::from_toml(&raw);
    }

    #[test]
    fn project_config_parser_never_panics_on_tomlish_fragments(
        raw in tomlish_strategy()
    ) {
        let _ = ProjectConfig::from_toml(&raw);
    }

    #[test]
    fn user_config_parser_never_panics_on_arbitrary_utf8(
        raw in arbitrary_utf8_strategy()
    ) {
        let _ = UserConfig::from_toml(&raw);
    }

    #[test]
    fn user_config_parser_never_panics_on_tomlish_fragments(
        raw in tomlish_strategy()
    ) {
        let _ = UserConfig::from_toml(&raw);
    }

    #[test]
    fn group_manifest_parser_never_panics_on_arbitrary_utf8(
        raw in arbitrary_utf8_strategy()
    ) {
        let _ = GroupManifest::from_toml(&raw);
    }

    #[test]
    fn group_manifest_parser_never_panics_on_tomlish_fragments(
        raw in tomlish_strategy()
    ) {
        let _ = GroupManifest::from_toml(&raw);
    }
}
