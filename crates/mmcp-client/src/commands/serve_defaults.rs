//! Default values for [`super::serve`].

use std::time::Duration;

/// Outer request-level bound on a single MCP sync tool call (`sync_fetch`,
/// `sync_pull`, `sync_push`): the engine build plus one fetch/pull/push
/// fan-out across every in-scope group.
///
/// Deliberately exceeds any per-subprocess git timeout further down the
/// stack, with headroom for the bounded-concurrency batches
/// `MAX_CONCURRENT_GROUP_TRANSFERS` still runs sequentially and for the
/// manifest/resolver calls that precede the fan-out, so this guard fires
/// only when a layer beneath the MCP handler hangs past its own bounds.
pub(super) const SYNC_SINGLE_OP_TIMEOUT: Duration = Duration::from_secs(300);

/// Outer request-level bound on the `sync` MCP tool (pull then push,
/// sequentially, in one call).
///
/// Twice [`SYNC_SINGLE_OP_TIMEOUT`]: `SyncEngine::sync` runs a full pull to
/// completion before starting the push, so the combined call's worst case
/// is the sum of both single-operation bounds, not a shared one.
pub(super) const SYNC_FULL_TIMEOUT: Duration = Duration::from_secs(600);
