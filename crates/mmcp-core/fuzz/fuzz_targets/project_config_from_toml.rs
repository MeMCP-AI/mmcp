//! Fuzz target for `ProjectConfig::from_toml`.
//!
//! `.mmcp.toml` at the project root passes through this parser on
//! every CLI invocation and every `bootstrap_context` call. A panic
//! here takes the whole client down before any tool runs.
//!
//! Invariant: `from_toml` must always return a `Result` regardless
//! of input bytes.

#![no_main]

use libfuzzer_sys::fuzz_target;
use mmcp_core::config::ProjectConfig;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let _ = ProjectConfig::from_toml(text);
});
