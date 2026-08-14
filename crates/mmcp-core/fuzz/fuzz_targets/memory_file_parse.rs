//! Fuzz target for `MemoryFile::parse`.
//!
//! mmcp's write paths (CLI `import`, `write_memory` MCP tool) funnel
//! every incoming memory through `MemoryFile::parse`, which fans out
//! into three deserializers (TOML, YAML, JSON). A parser panic on any
//! adversarial byte sequence takes the MCP session down. The proptest
//! harness in `tests/memory_parse_props.rs` runs a few hundred cases
//! per suite invocation; this fuzz target runs millions per CI hour.
//!
//! Invariants exercised by this target:
//! 1. `parse` on any `&str` built from `data` never panics.
//! 2. Successfully-parsed files round-trip through `to_string()` →
//!    `parse` and reconstruct the same frontmatter + format.
//!
//! Crashes dumped into `fuzz/artifacts/memory_file_parse/` are
//! reproducible via `cargo fuzz run memory_file_parse <artifact>`.

#![no_main]

use libfuzzer_sys::fuzz_target;
use mmcp_core::memory::MemoryFile;

fuzz_target!(|data: &[u8]| {
    // Invalid UTF-8 is rejected at the API boundary; fuzzing focuses
    // on the post-boundary parse path.
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };

    // Property 1: never panic.
    let parsed = match MemoryFile::parse(text) {
        Ok(f) => f,
        Err(_) => return,
    };

    // Property 2: rendering a parsed file and re-parsing must succeed
    // and preserve the frontmatter + format. Body equality is
    // intentionally omitted: gray_matter normalizes a single
    // leading newline out of the body, which is documented behavior
    // (covered by the unit suite).
    let Ok(rendered) = parsed.to_string() else {
        return;
    };
    let Ok(reparsed) = MemoryFile::parse(&rendered) else {
        panic!("rendered output failed to re-parse:\n{rendered}");
    };
    assert_eq!(reparsed.frontmatter, parsed.frontmatter);
    assert_eq!(reparsed.format, parsed.format);
});
