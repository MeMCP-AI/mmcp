//! Response-time output-size bounding for MCP tool responses.
//!
//! Peer of [`crate::memory::limits`], which bounds a memory's own
//! stored fields at WRITE time (see that module's own doc comment
//! for why it is a distinct concern). This module bounds a
//! different thing: how many bytes a single MCP tool RESPONSE
//! returns to the calling client, independent of how large the
//! underlying stored data legally is. A memory body that fits
//! inside [`crate::memory::MAX_BODY_LENGTH`] (512 KiB) still
//! overflows the calling MCP client's per-response ceiling when
//! read back whole, and a group whose individual records are all
//! small still overflows that same ceiling once an unfiltered
//! `list_memories` call returns hundreds of them.
//!
//! ## Evidence (issue #46)
//!
//! Measured live against this project's own mmcp mirror (see the
//! project memory
//! `list-memories-bootstrap-context-read-memory-responses-can-exceed-the-mcp-tool-output-size-ceiling-on-long-lived-or-high-subscription-projects`
//! for the full write-up):
//! - `list_memories` on the ~90-memory global group: roughly 60-62
//!   KiB, spilled to a side file by the calling harness rather than
//!   hard-failing.
//! - `list_memories` on this project's own ~115-123-memory project
//!   group: 73,747 bytes, a hard "exceeds maximum allowed tokens"
//!   failure.
//! - `list_memories` on a 471-memory project group (`hubedia`):
//!   308,678 bytes.
//! - a single `read_memory` body: up to 388,000 bytes observed; 58
//!   over-the-cap failures in one session, forcing agents to read
//!   the raw git objects on disk instead of the tool.
//!
//! [`DEFAULT_RESPONSE_BUDGET_BYTES`] sits below the smallest
//! observed spill (roughly 60 KiB) with close to 2x margin, so a
//! response sized to this budget stays clear of both the soft-spill
//! and hard-fail thresholds actually observed.
//!
//! ## Shared shape
//!
//! [`ResponseEnvelope`] is the ONE truncation/pagination shape used
//! by every tool that bounds its response below the full result
//! set: `read_memory` (byte truncation of the body, promoting the
//! caller toward `read_memory_body_sections`) and `list_memories`
//! (offset/limit pagination of the non-mandatory window). A caller
//! learns the pattern once instead of once per tool.

use serde::Serialize;

/// Default byte budget for a single MCP tool response payload.
/// `read_memory`'s inline body cap and `list_memories`'s default
/// page size are both derived from this one number. See the module
/// doc for the measured spill/failure thresholds it sits below.
pub const DEFAULT_RESPONSE_BUDGET_BYTES: usize = 32 * 1024;

/// Estimated TYPICAL serialized JSON size of one COMPACT
/// `list_memories` descriptor (`slug`, `path`, `name`, `kind`,
/// `mandatory`, plus object/array punctuation). This is a measured
/// average, not a worst-case bound: field lengths observed on this
/// project's own mmcp mirror average name ~52 chars, slug ~72 chars,
/// path ~74 chars (issue #46). The theoretical worst case, with
/// `name` at [`crate::memory::MAX_NAME_LENGTH`] (256 bytes) and
/// `slug` at mmcp-store's `MAX_SLUG_LENGTH` (256 bytes, re-encoded a
/// second time into `path`), plus JSON punctuation, runs closer to
/// 838 bytes, well above this constant. [`DEFAULT_LIST_MEMORIES_LIMIT`]
/// therefore fits the shared response budget for the typical record
/// sizes actually observed, not as a hard guarantee for a group of
/// unusually long slugs and names.
pub const COMPACT_RECORD_ESTIMATED_BYTES: usize = 512;

/// Default page size for `list_memories` pagination when the caller
/// sets `offset` and/or `limit` but omits an explicit `limit` value.
/// Derived from [`DEFAULT_RESPONSE_BUDGET_BYTES`] divided by one
/// compact record's estimated size, so the default page fits the
/// shared response budget with margin left over for the wrapper
/// object and the always-included mandatory set.
pub const DEFAULT_LIST_MEMORIES_LIMIT: usize =
    DEFAULT_RESPONSE_BUDGET_BYTES / COMPACT_RECORD_ESTIMATED_BYTES;

/// Hard upper clamp on a caller-supplied `limit`, independent of
/// [`DEFAULT_LIST_MEMORIES_LIMIT`]. Re-audited 2026-08-12: the
/// largest real group measured on this project's own mmcp mirror
/// carries 471 memories (the `hubedia`
/// architecture-cleanup-sweep group). 512 keeps roughly 1.1x
/// headroom above that observed maximum while still bounding a
/// pathological caller-supplied limit.
pub const MAX_LIST_MEMORIES_LIMIT: usize = 512;

// Compile-time invariants on the constants above: a violation fails
// the build rather than needing a runtime test to catch it. This
// checks only the constants' own arithmetic relationship. It does
// NOT prove a real response body always stays under budget:
// COMPACT_RECORD_ESTIMATED_BYTES is a measured-typical estimate, not
// a worst-case bound (see its own doc comment for the ~838-byte
// worst case it does not cover).
const _: () = assert!(MAX_LIST_MEMORIES_LIMIT >= DEFAULT_LIST_MEMORIES_LIMIT);
const _: () = assert!(
    DEFAULT_LIST_MEMORIES_LIMIT * COMPACT_RECORD_ESTIMATED_BYTES <= DEFAULT_RESPONSE_BUDGET_BYTES
);

/// Shared truncation/pagination envelope. See the module doc's
/// "Shared shape" section for which tools reuse it and why.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ResponseEnvelope {
    /// `true` exactly when `returned < total`. Computed, never
    /// caller-supplied, so an envelope can never claim `false` while
    /// actually under-returning: truncation is signalled, never
    /// silent.
    pub truncated: bool,
    /// The full size the caller has not necessarily seen all of.
    /// Bytes for `read_memory`'s body; item count for
    /// `list_memories`'s paginated (non-mandatory) window.
    pub total: usize,
    /// How much this response actually returned, same unit as
    /// `total`.
    pub returned: usize,
    /// Where to resume (byte offset or item offset) to get the
    /// rest, when `truncated`. `None` when nothing remains to fetch
    /// in this shape.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_offset: Option<usize>,
}

impl ResponseEnvelope {
    /// Build an envelope from the sizes actually observed.
    /// `truncated` is always computed from `total`/`returned`, never
    /// passed in, so a caller cannot construct an internally
    /// inconsistent envelope (e.g. `truncated: false` while
    /// `returned < total`).
    #[must_use]
    pub fn new(total: usize, returned: usize, next_offset: Option<usize>) -> Self {
        Self {
            truncated: returned < total,
            total,
            returned,
            next_offset,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelope_marks_truncated_when_returned_is_less_than_total() {
        let env = ResponseEnvelope::new(100, 40, Some(40));
        assert!(env.truncated);
        assert_eq!(env.total, 100);
        assert_eq!(env.returned, 40);
        assert_eq!(env.next_offset, Some(40));
    }

    #[test]
    fn envelope_not_truncated_when_everything_returned() {
        let env = ResponseEnvelope::new(10, 10, None);
        assert!(!env.truncated);
        assert_eq!(env.next_offset, None);
    }

    #[test]
    fn envelope_serializes_without_next_offset_when_absent() {
        let env = ResponseEnvelope::new(5, 5, None);
        let value = serde_json::to_value(env).expect("serialize");
        assert!(
            value.get("next_offset").is_none(),
            "next_offset must be omitted, not null, when absent: {value:?}"
        );
    }

    // The budget/limit relationships (default page fits the shared
    // budget; MAX_LIST_MEMORIES_LIMIT is at least the default) are
    // enforced above as compile-time `const _: () = assert!(...)`
    // checks rather than runtime tests here, per clippy's
    // `assertions_on_constants` lint: a violation fails the build
    // itself, a strictly earlier and stronger signal than a test
    // failure.
}
