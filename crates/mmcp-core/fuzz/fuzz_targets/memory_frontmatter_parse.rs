//! Fuzz target for `parse_frontmatter`.
//!
//! `parse_frontmatter` exists to give frontmatter-only callers a
//! cheaper alternative to `MemoryFile::parse` (see the
//! `memory_file_parse` fuzz target's own rationale). Its whole
//! contract is: identical results to `MemoryFile::parse(..).frontmatter`
//! for every input, valid or malformed, without ever building the
//! body. This target proves that equivalence never breaks and the
//! new fence-scanning path never panics on adversarial byte
//! sequences.
//!
//! Crashes dumped into `fuzz/artifacts/memory_frontmatter_parse/` are
//! reproducible via `cargo fuzz run memory_frontmatter_parse <artifact>`.

#![no_main]

use libfuzzer_sys::fuzz_target;
use mmcp_core::memory::{MemoryFile, parse_frontmatter};

fuzz_target!(|data: &[u8]| {
    // Invalid UTF-8 is rejected at the API boundary; fuzzing focuses
    // on the post-boundary parse path.
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };

    // Property 1: never panic, matching the full parser's guarantee.
    let full = MemoryFile::parse(text);
    let fast = parse_frontmatter(text);

    // Property 2: success/failure agree, and on success the parsed
    // frontmatter is byte-for-byte identical.
    match (full, fast) {
        (Ok(full), Ok(fast)) => {
            assert_eq!(
                fast, full.frontmatter,
                "parse_frontmatter diverged from MemoryFile::parse for:\n{text}"
            );
        }
        (Err(_), Err(_)) => {}
        (full, fast) => {
            panic!(
                "parse_frontmatter and MemoryFile::parse disagreed on success for:\n{text}\nfull: {:?}\nfast: {:?}",
                full.is_ok(),
                fast.is_ok()
            );
        }
    }
});
