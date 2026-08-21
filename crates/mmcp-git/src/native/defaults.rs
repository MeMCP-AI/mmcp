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

/// SSH flag appended to an ambient `GIT_SSH_COMMAND` value that
/// `crate::native::repo_ops::suppress_interactive_prompts` finds
/// already set: SSH fails immediately instead of prompting for a
/// host-key confirmation or a key passphrase. Appending, rather than
/// replacing, keeps the operator's own custom identity or tool (a
/// deploy key, `IdentitiesOnly=yes`) intact. Assumes the ambient
/// command is OpenSSH-compatible; a non-OpenSSH ambient command (a
/// `plink`-based Windows setup) may not accept this flag as intended.
pub const SSH_BATCH_MODE_FLAG: &str = "-o BatchMode=yes";

/// Default value injected into `GIT_SSH_COMMAND` when neither the
/// caller nor the ambient process environment already sets one.
///
/// Plain `ssh` plus [`SSH_BATCH_MODE_FLAG`]: `~/.ssh/config` still
/// governs identity files and per-host options since this still
/// invokes plain `ssh`.
pub const DEFAULT_SSH_COMMAND_BATCH_MODE: &str = "ssh -o BatchMode=yes";
