//! Pagination-only output-size bounding for MCP tool responses.
//!
//! Peer of [`crate::memory::limits`], which bounds a memory's own stored fields at WRITE time.
//! This module bounds a different thing: how many items a single MCP tool RESPONSE returns to the calling client.
//! That is independent of how large the underlying stored data legally is.
//! A group whose individual records are all small can still overflow a client's per-response ceiling.
//! An unfiltered `list_memories` call returning hundreds of them is enough.
//! `read_memory` itself carries no bound here: a memory body reads back whole regardless of size.
//! [`crate::memory::limits::MCP_CLIENT_RESULT_CEILING_BYTES`] is the one owner of the write-time body ceiling
//! this module's page-size defaults derive from.
//!
//! ## Evidence (measured response sizes)
//!
//! Measured live against this project's own mmcp mirror:
//! - `list_memories` on the ~90-memory global group: roughly 60-62 KiB.
//!   Spilled to a side file by the calling harness rather than hard-failing.
//! - `list_memories` on this project's own ~115-123-memory project group: 73,747 bytes.
//!   A hard "exceeds maximum allowed tokens" failure.
//! - `list_memories` on a 471-memory project group (`hubedia`): 308,678 bytes.
//!
//! ## Shared shape
//!
//! [`ResponseEnvelope`] is the ONE pagination shape every tool uses to page its response below the full result set.
//! Tools using it: `list_memories`, `list_versions`, `list_groups`, and `list_milestones`.
//! `list_memories` pages its non-mandatory window by offset/limit.
//! A caller learns the pattern once instead of once per tool.

use serde::Serialize;

/// Estimated TYPICAL serialized JSON size of one COMPACT `list_memories` descriptor.
/// Fields: `slug`, `path`, `name`, `kind`, `mandatory`, plus object/array punctuation.
/// This is a measured average, not a worst-case bound.
/// Field lengths observed on this project's own mirror average name ~52 chars.
/// Slug averages ~72 chars, path ~74 chars.
/// See the measured evidence in the module doc above.
/// The theoretical worst case runs closer to 838 bytes, well above this constant.
/// `name` and `slug` each reach their 256-byte maximum in that case.
/// `slug` is also re-encoded a second time into `path`, plus JSON punctuation.
/// [`DEFAULT_LIST_MEMORIES_LIMIT`] therefore fits the client result ceiling for typical record sizes.
/// It is not a hard guarantee for a group of unusually long slugs and names.
pub const COMPACT_RECORD_ESTIMATED_BYTES: usize = 512;

/// Bytes withheld from the ceiling before sizing the default page.
/// Covers the envelope wrapper's own JSON punctuation and a typical mandatory set (about 8 compact records' worth).
/// The mandatory set rides in unconditionally, outside pagination, so it is not itself bounded by this reserve.
/// A group whose mandatory set alone exceeds this reserve can still push a default-limit response over the ceiling.
pub const LIST_MEMORIES_RESERVE_BYTES: usize = 4096;

/// Default page size for `list_memories` pagination.
/// Used when the caller sets `offset` and/or `limit` but omits an explicit `limit` value.
/// Derived from [`crate::memory::limits::MCP_CLIENT_RESULT_CEILING_BYTES`] over one compact record's estimated size,
/// after withholding [`LIST_MEMORIES_RESERVE_BYTES`].
/// The default page plus that reserve fits the client result ceiling for a typical mandatory set.
pub const DEFAULT_LIST_MEMORIES_LIMIT: usize = (crate::memory::limits::MCP_CLIENT_RESULT_CEILING_BYTES
    - LIST_MEMORIES_RESERVE_BYTES)
    / COMPACT_RECORD_ESTIMATED_BYTES;

/// Hard upper clamp on a caller-supplied `limit`, independent of [`DEFAULT_LIST_MEMORIES_LIMIT`].
/// The largest real group measured on this project's own mmcp mirror carries 471 memories.
/// That is the `hubedia` architecture-cleanup-sweep group.
/// 512 keeps roughly 1.1x headroom above that observed maximum.
/// It still bounds a pathological caller-supplied limit.
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
    DEFAULT_LIST_MEMORIES_LIMIT * COMPACT_RECORD_ESTIMATED_BYTES + LIST_MEMORIES_RESERVE_BYTES
        <= crate::memory::limits::MCP_CLIENT_RESULT_CEILING_BYTES
);

/// Shared pagination envelope.
/// See the module doc's "Shared shape" section for which tools reuse it and why.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ResponseEnvelope {
    /// `true` exactly when `returned < total`. Computed, never
    /// caller-supplied, so an envelope can never claim `false` while
    /// actually under-returning: truncation is signalled, never
    /// silent.
    pub truncated: bool,
    /// The full item count the caller has not necessarily seen all of.
    pub total: usize,
    /// How much this response actually returned, same unit as
    /// `total`.
    pub returned: usize,
    /// Item offset to resume from to get the rest, when `truncated`.
    /// `None` when nothing remains to fetch in this shape.
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
    #![allow(clippy::unwrap_used, clippy::expect_used)]
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
