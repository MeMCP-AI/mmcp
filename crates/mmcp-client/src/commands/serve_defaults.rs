//! Default values for [`super::serve`].

use std::time::Duration;

/// Outer request-level bound on a single MCP sync tool call (`sync_fetch`,
/// `sync_pull`, `sync_push`): the engine build plus one fetch/pull/push
/// fan-out across every in-scope group.
///
/// `mmcp_git::native`'s own per-subprocess transport bound (`fetch`/`push`,
/// 300s each; not directly reachable from this crate, so restated here
/// rather than imported) already recurs per group: `SyncEngine` fans a
/// fetch/push out across every in-scope group in batches of
/// `MAX_CONCURRENT_GROUP_TRANSFERS` (currently 6) run concurrently within a
/// batch but sequentially across batches, and never retries a failed
/// transfer internally (a hang is surfaced, never silently retried). One
/// full batch of groups simultaneously bumping against that 300s bound is
/// therefore the realistic worst case this outer guard has to outlast, not
/// a single subprocess call: 6 x 300s = 1800s. Set at exactly that bound,
/// which leaves the batch itself no slack for `build_engine` or the
/// manifest/resolver reads that precede it; a genuinely healthy single
/// batch normally finishes in a small fraction of 300s per transfer, so
/// this still fires only when a layer beneath the MCP handler hangs well
/// past its own bound, never on ordinary multi-group latency. A group
/// count exceeding one batch (more than 6 in-scope groups) needs a second
/// sequential batch and pushes past this bound even with no single hang;
/// track that as a known scaling limit rather than growing this constant
/// further speculatively.
pub(super) const SYNC_SINGLE_OP_TIMEOUT: Duration = Duration::from_secs(1800);

/// Outer request-level bound on the `sync` MCP tool (pull then push,
/// sequentially, in one call).
///
/// Twice [`SYNC_SINGLE_OP_TIMEOUT`]: `SyncEngine::sync` runs a full pull to
/// completion before starting the push, so the combined call's worst case
/// is the sum of both single-operation bounds, not a shared one.
pub(super) const SYNC_FULL_TIMEOUT: Duration = Duration::from_secs(3600);
