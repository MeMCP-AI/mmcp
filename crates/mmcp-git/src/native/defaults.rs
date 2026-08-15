//! Default values for the native `gix` backend.

/// Maximum number of open [`gix::ThreadSafeRepository`] handles
/// [`crate::native::NativeBackend`] keeps resident in `repo_cache` at
/// once.
///
/// The cache exists purely to skip repeat `gix` discovery /
/// config-parse / ODB-mount cost, not to guarantee residency: every
/// live caller already holds its own clone of the handle it is using
/// (see `NativeBackend::open_repo`'s doc comment), so an entry evicted
/// under this bound only means the next `open_repo` call for that path
/// re-pays discovery cost, never a correctness issue. Bounded instead
/// of unbounded so a long-lived server process touching many distinct
/// group repositories over its lifetime cannot grow this map without
/// limit.
pub const REPO_CACHE_MAX_ENTRIES: u64 = 512;
