//! Fuzz target for `GroupManifest::from_toml`.
//!
//! The group manifest sits inside every bare repo under
//! `~/.mmcp/repos/<uuid>.git` and is parsed on every group scan.
//! A panicking parser propagates all the way up to
//! `mmcp_store::groups::GroupIndex::build`, which runs at MCP
//! session start.

#![no_main]

use libfuzzer_sys::fuzz_target;
use mmcp_core::manifest::GroupManifest;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let _ = GroupManifest::from_toml(text);
});
