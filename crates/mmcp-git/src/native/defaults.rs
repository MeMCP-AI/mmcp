//! Default values for the native `gix` backend.

use std::time::Duration;

/// Upper bound for a `git fetch` or `git push` subprocess.
///
/// An incremental transfer over a slow or lossy connection can
/// legitimately take minutes. Unbounded, a stalled remote used to pin
/// the calling task (and, before the `tokio::process` migration, an
/// entire blocking-pool thread plus the child process) forever. Five
/// minutes is generous for a real transfer of a reasonably sized group
/// repository while still turning an indefinite hang into a bounded
/// failure with a clear error.
pub const FETCH_PUSH_TIMEOUT: Duration = Duration::from_secs(5 * 60);

/// Upper bound for a `git clone` subprocess.
///
/// A clone transfers the full repository history in one shot, which
/// can be substantially larger than a single `fetch`/`push`
/// increment, so it gets a longer bound than [`FETCH_PUSH_TIMEOUT`].
pub const CLONE_TIMEOUT: Duration = Duration::from_secs(15 * 60);

/// Upper bound for a purely local `git remote set-url`/`git remote
/// add` subprocess (see `ensure_remote` in
/// [`crate::native::repo_ops`]).
///
/// No network I/O is involved, so this defends only against
/// pathological local contention (a stale `.git/index.lock` held by
/// another process): a healthy local git-config edit completes in
/// well under a second.
pub const LOCAL_GIT_OP_TIMEOUT: Duration = Duration::from_secs(30);

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

/// Size, in bytes, of the `gix` decoded-object cache set on every
/// thread-local [`gix::Repository`] derived from a cached repo handle.
///
/// `gix::Repository::object_cache_size_if_unset` takes a byte budget
/// for a `MemoryCappedHashmap` of fully decoded objects (unset by
/// default in the pinned fork). A single operation here (a `read_files`
/// batch, a recursive tree listing) re-decodes the same root tree and
/// `memories/` tree from the on-disk object database on every visit;
/// this budget comfortably holds a few hundred small tree and blob
/// objects at this project's own repo scale, so those re-visits become
/// cache hits within one `spawn_blocking` call.
pub const OBJECT_CACHE_SIZE_BYTES: usize = 1024 * 1024;

/// SSH flag appended to an ambient `GIT_SSH_COMMAND` value, or to a
/// `core.sshCommand` git-config value, that
/// `crate::native::repo_ops::suppress_interactive_prompts` finds
/// already set: SSH fails immediately instead of prompting for a
/// host-key confirmation or a key passphrase. Appending, rather than
/// replacing, keeps the operator's own custom identity or tool (a
/// deploy key, `IdentitiesOnly=yes`) intact. Assumes the ambient or
/// configured command is OpenSSH-compatible; a non-OpenSSH command (a
/// `plink`-based Windows setup) may not accept this flag as intended.
pub const SSH_BATCH_MODE_FLAG: &str = "-o BatchMode=yes";

/// Default value injected into `GIT_SSH_COMMAND` when none of the
/// caller, the ambient process environment, or the `core.sshCommand`
/// git config sets one.
///
/// Plain `ssh` plus [`SSH_BATCH_MODE_FLAG`]: `~/.ssh/config` still
/// governs identity files and per-host options since this still
/// invokes plain `ssh`.
pub const DEFAULT_SSH_COMMAND_BATCH_MODE: &str = "ssh -o BatchMode=yes";
