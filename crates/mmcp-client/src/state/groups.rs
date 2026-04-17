//! Group index shim — moved to `mmcp-store` as part of FR-020.
//!
//! This file stays in `mmcp-client` so `crate::state::groups` /
//! `crate::state::{GroupEntry, GroupIndex}` imports keep
//! resolving; in commit 8 it disappears along with
//! `mmcp-client/src/lib.rs` and the remaining call sites rebase
//! on `mmcp_store::groups` directly.

pub use mmcp_store::groups::{GroupEntry, GroupIndex};
