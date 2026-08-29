//! Repository operations against a bare git repo.
//!
//! Every function that needs an open repository takes an
//! already-opened `&gix::Repository`: [`crate::native::NativeBackend`]
//! owns the cached [`gix::ThreadSafeRepository`] handle and hands out
//! a thread-local view per call, so this module never re-opens a repo
//! itself. The `gix`-based read/write functions are synchronous and
//! are invoked from `spawn_blocking` inside the async backend, the
//! same as ever.
//!
//! [`clone`], [`fetch`], [`push`], and `ensure_remote` are the
//! exception: they shell out to the `git` binary for the actual
//! network transfer, so they are genuinely `async fn` built on
//! `tokio::process::Command`. Each subprocess call runs under
//! `tokio::time::timeout`; on expiry the child is explicitly killed
//! and reaped before the function returns
//! [`GitError::Timeout`](crate::error::GitError::Timeout), so a dead
//! or unresponsive remote fails within a bounded window instead of
//! pinning the calling task (or, with the old `spawn_blocking` design,
//! an entire blocking-pool thread) indefinitely.

use std::ffi::OsString;
use std::path::Path;
use std::process::{Output, Stdio};
use std::time::Duration;

use bytes::Bytes;
use gix::bstr::{BStr, BString};
use gix::object::tree::editor::ToComponents;
use gix::objs::tree::EntryKind;
use tokio::io::AsyncReadExt;
use tokio::process::{Child, Command};

use crate::error::GitError;
use crate::native::defaults;
use crate::types::{CommitMeta, CommitSpec, Credentials, FastForwardOutcome, PushReport, Rev};

/// Wrap a `gix` failure while resolving a revision to a commit, or
/// while walking the commit graph (ancestry checks, history walks).
fn resolve_rev_err<E: std::error::Error + Send + Sync + 'static>(err: E) -> GitError {
    GitError::ResolveRev {
        source: Box::new(err),
    }
}

/// Wrap a `gix` failure while reading a blob or descending a tree to
/// find one.
fn read_blob_err<E: std::error::Error + Send + Sync + 'static>(err: E) -> GitError {
    GitError::ReadBlob {
        source: Box::new(err),
    }
}

/// Wrap a `gix` failure while building or writing a new commit object.
fn commit_err<E: std::error::Error + Send + Sync + 'static>(err: E) -> GitError {
    GitError::Commit {
        source: Box::new(err),
    }
}

/// Build a [`GitError::Commit`] from an ad hoc message, for invariant
/// violations that have no underlying `gix` error to wrap.
fn commit_msg_err(msg: impl Into<String>) -> GitError {
    GitError::Commit {
        source: msg.into().into(),
    }
}

/// Wrap a `gix` failure while creating or updating a ref: a branch, a
/// tag, or the target of a fast-forward.
fn ref_update_err<E: std::error::Error + Send + Sync + 'static>(
    name: impl Into<String>,
) -> impl FnOnce(E) -> GitError {
    move |err| GitError::RefUpdate {
        name: name.into(),
        source: Box::new(err),
    }
}

/// Name of the env var that overrides the `git` binary path.
///
/// Lets operators point mmcp at a specific git install without
/// touching `PATH`: useful on Windows where Git for Windows often
/// lives under `C:\Program Files\Git\cmd\git.exe`, and in containers
/// where multiple git versions coexist.
pub const GIT_BIN_ENV: &str = "MMCP_GIT_BIN";

/// Resolve the `git` binary path, honouring the `MMCP_GIT_BIN` env
/// override. Returns `git` when the env var is unset or empty so
/// the OS's `PATH` lookup kicks in.
pub(crate) fn git_binary() -> OsString {
    std::env::var_os(GIT_BIN_ENV)
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| OsString::from("git"))
}

/// Apply [`Credentials`] to a `git` command via environment variables,
/// never argv, and unconditionally force the subprocess headless via
/// [`suppress_interactive_prompts`].
///
/// [`Credentials::None`] sets no credential of its own: the ambient
/// git env (SSH agent, credential helper, `.netrc`, and an ambient
/// `GIT_SSH_COMMAND` if one is set) still decides *which* identity
/// authenticates. Headless suppression still applies on this arm: an
/// mmcp caller (an MCP server process, the sync engine) has no human
/// present to answer an interactive prompt, so a missing explicit
/// credential fails fast instead of hanging. See
/// [`suppress_interactive_prompts`] for exactly how an ambient
/// `GIT_SSH_COMMAND`, or a `core.sshCommand` git-config value, is
/// preserved rather than replaced.
///
/// For `BearerHttp`, sets `GIT_CONFIG_COUNT`/`GIT_CONFIG_KEY_0`/
/// `GIT_CONFIG_VALUE_0` (git's environment-based config protocol,
/// git >= 2.31), the env equivalent of `-c http.extraHeader=Authorization:
/// Bearer <token>`. Passing it via `-c` puts the token in the
/// subprocess's argv, readable by any local process listing
/// (`/proc/<pid>/cmdline`, Process Explorer) for the subprocess's
/// lifetime; the environment-variable form keeps it out of argv. The
/// token still appears in the child's environment block
/// (`/proc/<pid>/environ`), the same exposure `SshCommand` below
/// already accepts for its own value.
///
/// For `SshCommand`, sets `GIT_SSH_COMMAND` to the caller-supplied
/// value before [`suppress_interactive_prompts`] runs, so that call's
/// own default-`GIT_SSH_COMMAND` injection sees the variable already
/// set and leaves it untouched.
///
/// Any `-c` flags must appear *before* the git subcommand, so this
/// helper is called on a freshly-constructed `Command` before its
/// `.arg("clone")` / `.arg("fetch")` / etc.
///
/// `repo_path` scopes [`suppress_interactive_prompts`]'s
/// `core.sshCommand` lookup: `Some` for `fetch`/`push` against an
/// already-cloned repo, `None` for `clone`, which has no repo yet to
/// scope a local config lookup to.
async fn apply_credentials(cmd: &mut Command, creds: &Credentials, repo_path: Option<&Path>) {
    match creds {
        Credentials::None => {}
        Credentials::BearerHttp(token) => {
            cmd.env("GIT_CONFIG_COUNT", "1");
            cmd.env("GIT_CONFIG_KEY_0", "http.extraHeader");
            cmd.env(
                "GIT_CONFIG_VALUE_0",
                format!("Authorization: Bearer {token}"),
            );
        }
        Credentials::SshCommand(value) => {
            cmd.env("GIT_SSH_COMMAND", value);
        }
    }
    suppress_interactive_prompts(cmd, repo_path).await;
}

/// Force every `git` subprocess headless: suppress the terminal
/// prompt, `GIT_ASKPASS`, any configured `credential.helper` (on
/// Windows, typically Git Credential Manager, which pops a real
/// desktop dialog), and OpenSSH's own interactive prompts (host-key
/// confirmation, key-passphrase entry) for a `git@host:path`-style
/// SSH URL. Called unconditionally from every [`apply_credentials`]
/// arm, [`Credentials::None`] included: an mmcp caller has no human
/// present to answer any of these, so every credential shape fails
/// fast rather than hangs. The remaining headless axis, stdin itself,
/// is nulled separately in [`run_git_subprocess`]: `mmcp serve` is an
/// MCP stdio server, so an inherited stdin would otherwise be the
/// live JSON-RPC request stream.
///
/// `-c credential.helper=` (empty value) disables every configured
/// helper for this invocation only, without touching the user's
/// global git config. `GIT_TERMINAL_PROMPT=0` stops git's own
/// built-in terminal prompt. `GIT_ASKPASS=""` (empty program path)
/// makes git treat askpass as unconfigured rather than trying to
/// execute an empty command.
///
/// `GIT_TERMINAL_PROMPT`/`GIT_ASKPASS`/`credential.helper` cover only
/// git's own prompt machinery, not the `ssh` subprocess git spawns for
/// a `git@host:path` URL. This function governs `GIT_SSH_COMMAND` in
/// four tiers, checked in order, mirroring git's own real precedence
/// between the environment variable and the `core.sshCommand` config
/// key:
///
/// 1. The caller already set `GIT_SSH_COMMAND` on this `Command`
///    (checked via `Command::get_envs`): [`Credentials::SshCommand`]'s
///    own value is left completely untouched.
/// 2. No builder value, but the ambient process environment
///    (`std::env::var_os`) already has a non-empty `GIT_SSH_COMMAND`:
///    that value is preserved and extended with
///    [`defaults::SSH_BATCH_MODE_FLAG`] appended, rather than replaced
///    outright, so an operator's own custom identity or tool (a
///    deploy key, a `plink`-based Windows setup) keeps working while
///    the interactive-hang risk still closes for the common case.
/// 3. No builder or ambient value, but `core.sshCommand` resolves to a
///    non-empty value via [`read_core_ssh_command`], scoped to
///    `repo_path` per that function's own doc comment: that value is
///    preserved and extended the same way as tier 2, so a
///    config-only SSH client override (for example a working native
///    OpenSSH client pointed at by `core.sshCommand`, on a host where
///    the bare `ssh` resolved via `PATH` is a different, non-working
///    client) is honored instead of silently overridden.
/// 4. None of the above: defaults to
///    [`defaults::DEFAULT_SSH_COMMAND_BATCH_MODE`].
///
/// `BatchMode=yes` turns an unanswerable prompt into an immediate
/// failure with a clear stderr message instead of an indefinite hang,
/// without weakening host-key verification: no `StrictHostKeyChecking`
/// override is added.
///
/// One known, deliberately unaddressed gap: appending an OpenSSH `-o`
/// flag to a non-OpenSSH ambient or `core.sshCommand` value (tiers 2
/// and 3) may not behave as intended, since `-o` is an OpenSSH-specific
/// flag; there is no portable way to express `BatchMode` for an
/// arbitrary configured tool.
async fn suppress_interactive_prompts(cmd: &mut Command, repo_path: Option<&Path>) {
    cmd.arg("-c").arg("credential.helper=");
    cmd.env("GIT_TERMINAL_PROMPT", "0");
    cmd.env("GIT_ASKPASS", "");

    let builder_set_ssh_command = cmd
        .as_std()
        .get_envs()
        .any(|(key, _)| key == std::ffi::OsStr::new("GIT_SSH_COMMAND"));
    if builder_set_ssh_command {
        return;
    }

    let ambient = std::env::var_os("GIT_SSH_COMMAND");
    // Tier 3 only runs a git-config subprocess when tier 2 has nothing:
    // the common ambient/builder cases never pay the extra spawn.
    let core_ssh_command = if ambient.as_ref().is_some_and(|v| !v.is_empty()) {
        None
    } else {
        read_core_ssh_command(repo_path).await
    };
    let resolved = resolve_ssh_command(ambient, core_ssh_command);
    cmd.env("GIT_SSH_COMMAND", resolved);
}

/// Resolve tiers 2 through 4 of [`suppress_interactive_prompts`]'s
/// `GIT_SSH_COMMAND` logic: `ambient` is the caller's own
/// `std::env::var_os("GIT_SSH_COMMAND")` read, `core_ssh_command` is
/// [`read_core_ssh_command`]'s result; both are taken as parameters
/// rather than read internally so this pure resolution step is
/// testable without mutating real process environment state (this
/// crate forbids `unsafe` code, and `std::env::set_var` requires it)
/// or spawning a real `git config` subprocess.
///
/// The first non-empty value, checked `ambient` then
/// `core_ssh_command`, is extended with
/// [`defaults::SSH_BATCH_MODE_FLAG`] appended. With neither, falls
/// back to [`defaults::DEFAULT_SSH_COMMAND_BATCH_MODE`].
fn resolve_ssh_command(ambient: Option<OsString>, core_ssh_command: Option<OsString>) -> OsString {
    for candidate in [ambient, core_ssh_command].into_iter().flatten() {
        if !candidate.is_empty() {
            return extend_with_batch_mode(candidate);
        }
    }
    OsString::from(defaults::DEFAULT_SSH_COMMAND_BATCH_MODE)
}

/// Append [`defaults::SSH_BATCH_MODE_FLAG`] to `value`, space-separated.
fn extend_with_batch_mode(value: OsString) -> OsString {
    let mut extended = value;
    extended.push(" ");
    extended.push(defaults::SSH_BATCH_MODE_FLAG);
    extended
}

/// Read `core.sshCommand` the way `git` itself resolves it for
/// `repo_path`: local repo config overriding global/system, via
/// `git -C <repo_path> config --get core.sshCommand`. `None` (no repo
/// exists yet, e.g. before a `clone`) scopes to `--global` only, so an
/// unrelated current-working-directory repository's local config is
/// never consulted.
///
/// Bounded by [`defaults::LOCAL_GIT_OP_TIMEOUT`], the same bound
/// `ensure_remote`'s local `git remote` housekeeping uses: purely
/// local config resolution, no network I/O.
///
/// Every outcome resolves to `None` short of a genuine value.
/// [`suppress_interactive_prompts`] always falls through to its next tier.
///
/// `git config --get`'s exit code `1` means the key is unset and logs nothing.
/// A spawn failure or a timeout is a genuine local malfunction, logged at `warn`.
/// So a silently timed-out config read still surfaces to the operator.
async fn read_core_ssh_command(repo_path: Option<&Path>) -> Option<OsString> {
    let cmd = build_core_ssh_command_query(repo_path);
    let output = match run_git_subprocess(
        cmd,
        defaults::LOCAL_GIT_OP_TIMEOUT,
        "config-get",
        "core.sshCommand",
    )
    .await
    {
        Ok(output) => output,
        Err(GitError::Timeout { timeout_secs, .. }) => {
            tracing::warn!(
                timeout_secs,
                "git config --get core.sshCommand timed out; falling back to the next \
                 GIT_SSH_COMMAND tier"
            );
            return None;
        }
        Err(err) => {
            tracing::warn!(
                error = %err,
                "git config --get core.sshCommand failed to spawn or run; falling back \
                 to the next GIT_SSH_COMMAND tier"
            );
            return None;
        }
    };
    parse_core_ssh_command_output(output.status.success(), &output.stdout)
}

/// Build the `git config --get core.sshCommand` invocation
/// [`read_core_ssh_command`] runs, scoped per `repo_path` per that
/// function's own doc comment.
fn build_core_ssh_command_query(repo_path: Option<&Path>) -> Command {
    let mut cmd = Command::new(git_binary());
    match repo_path {
        Some(path) => {
            cmd.arg("-C").arg(path).arg("config").arg("--get");
        }
        None => {
            cmd.arg("config").arg("--get").arg("--global");
        }
    }
    cmd.arg("core.sshCommand");
    cmd
}

/// Parse `git config --get core.sshCommand`'s outcome: `success` is
/// its exit status, `stdout` its captured output. A failed exit
/// (`1` for an unset key, or any other nonzero code) yields `None`.
/// A successful exit with empty or whitespace-only stdout also yields
/// `None`; otherwise the trailing newline `git config --get` always
/// emits is stripped and the remaining value returned.
fn parse_core_ssh_command_output(success: bool, stdout: &[u8]) -> Option<OsString> {
    if !success {
        return None;
    }
    let text = String::from_utf8_lossy(stdout);
    let trimmed = text.trim_end_matches(['\n', '\r']);
    if trimmed.is_empty() {
        None
    } else {
        Some(OsString::from(trimmed))
    }
}

/// Initialize a bare repository at `path`, idempotent.
pub fn init_bare(path: &Path) -> Result<(), GitError> {
    if path.exists() {
        return Ok(());
    }
    gix::init_bare(path).map_err(|e| GitError::OpenRepo {
        path: path.to_string_lossy().into_owned(),
        source: Box::new(e),
    })?;
    // `gix::init_bare` points HEAD at the host's `init.defaultBranch`
    // (often `master`), but mmcp commits to `main`. Pin HEAD to `main`
    // so HEAD resolves to the branch mmcp actually writes, regardless of
    // the host git config: otherwise the repo looks empty-HEAD and the
    // group scan skips it.
    std::fs::write(
        path.join("HEAD"),
        format!("ref: refs/heads/{}\n", mmcp_core::conventions::MAIN_BRANCH),
    )?;
    Ok(())
}

/// Run `cmd` to completion, capturing its stdout/stderr while it
/// executes, bounded by `timeout`.
///
/// On expiry, explicitly kills the child and waits for it to actually
/// exit before returning [`GitError::Timeout`], so no child process
/// is ever left running past this call: `kill_on_drop` below is a
/// backstop for a panic or an early return elsewhere, not the
/// mechanism this path relies on. `op`/`target` label the error only;
/// they do not affect execution.
///
/// `stdin` is explicitly nulled: `mmcp serve` is an MCP stdio server,
/// so the parent's own stdin is the live JSON-RPC request stream.
/// `tokio::process::Command::spawn` inherits the parent's stdin
/// unless told otherwise, so this call sets it explicitly, keeping
/// every git subprocess headless on this axis too, matching
/// [`suppress_interactive_prompts`]'s own promise for the rest of
/// git's interactive surface.
async fn run_git_subprocess(
    mut cmd: Command,
    timeout: Duration,
    op: &'static str,
    target: &str,
) -> Result<Output, GitError> {
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    cmd.kill_on_drop(true);
    let mut child = cmd.spawn()?;
    wait_with_timeout(&mut child, timeout, op, target).await
}

/// Drain `child`'s stdout/stderr while waiting for it to exit, bounded
/// by `timeout`. Split out from [`run_git_subprocess`] so a test can
/// drive an already-spawned child directly and observe, via
/// `Child::try_wait`, that a timed-out child was genuinely reaped.
///
/// Stdout and stderr are read concurrently with the wait, not after
/// it: a child that fills its stderr pipe (git's own progress output
/// on a large transfer) before exiting would otherwise block on a
/// full OS pipe buffer, reintroducing exactly the kind of hang this
/// function exists to bound.
async fn wait_with_timeout(
    child: &mut Child,
    timeout: Duration,
    op: &'static str,
    target: &str,
) -> Result<Output, GitError> {
    // Every caller (`run_git_subprocess`, and the timeout tests that
    // drive this function directly) pipes stdout/stderr before
    // spawning; a `None` here means a caller broke that contract, an
    // unrecoverable programming error rather than a runtime condition
    // to route through `Result`.
    #[allow(clippy::expect_used)]
    let mut stdout_pipe = child
        .stdout
        .take()
        .expect("child must have been spawned with stdout piped");
    #[allow(clippy::expect_used)]
    let mut stderr_pipe = child
        .stderr
        .take()
        .expect("child must have been spawned with stderr piped");
    let mut stdout_buf = Vec::new();
    let mut stderr_buf = Vec::new();

    let collect = async {
        let (stdout_res, stderr_res, status_res) = tokio::join!(
            stdout_pipe.read_to_end(&mut stdout_buf),
            stderr_pipe.read_to_end(&mut stderr_buf),
            child.wait(),
        );
        stdout_res?;
        stderr_res?;
        status_res
    };

    match tokio::time::timeout(timeout, collect).await {
        Ok(Ok(status)) => Ok(Output {
            status,
            stdout: stdout_buf,
            stderr: stderr_buf,
        }),
        Ok(Err(io_err)) => Err(GitError::Io(io_err)),
        Err(_elapsed) => {
            child.kill().await?;
            // Reap so the OS process table entry is actually gone,
            // not merely signalled, before this returns.
            let _ = child.wait().await;
            Err(GitError::timeout(op, target, timeout))
        }
    }
}

/// Clone `remote_url` into `dst` via the user-installed git binary,
/// bounded by [`defaults::CLONE_TIMEOUT`].
pub async fn clone(remote_url: &str, dst: &Path, creds: &Credentials) -> Result<(), GitError> {
    let mut cmd = Command::new(git_binary());
    apply_credentials(&mut cmd, creds, None).await;
    cmd.arg("clone").arg(remote_url).arg(dst);
    let output = run_git_subprocess(cmd, defaults::CLONE_TIMEOUT, "clone", remote_url).await?;
    if !output.status.success() {
        return Err(GitError::transport(
            "clone",
            remote_url,
            String::from_utf8_lossy(&output.stderr).into_owned(),
        ));
    }
    Ok(())
}

/// Fetch `refspecs` from `remote_url` into the bare repo at
/// `repo_path`, bounded by [`defaults::FETCH_PUSH_TIMEOUT`]. An
/// `origin` remote is configured on the fly so subsequent fetches
/// reuse it.
pub async fn fetch(
    repo_path: &Path,
    remote_url: &str,
    refspecs: &[String],
    creds: &Credentials,
) -> Result<(), GitError> {
    ensure_remote(repo_path, remote_url).await?;
    let mut cmd = Command::new(git_binary());
    apply_credentials(&mut cmd, creds, Some(repo_path)).await;
    cmd.arg("-C").arg(repo_path).arg("fetch").arg("origin");
    for spec in refspecs {
        cmd.arg(spec);
    }
    let output = run_git_subprocess(cmd, defaults::FETCH_PUSH_TIMEOUT, "fetch", remote_url).await?;
    if !output.status.success() {
        return Err(GitError::transport(
            "fetch",
            remote_url,
            String::from_utf8_lossy(&output.stderr).into_owned(),
        ));
    }
    Ok(())
}

/// Push `refspecs` (local, remote, force) to `remote_url` against the
/// bare repo at `repo_path`, bounded by
/// [`defaults::FETCH_PUSH_TIMEOUT`].
///
/// Callers run [`preflight_local_refs`] against the repo's open
/// `gix::Repository` handle themselves before calling this: that
/// check needs the synchronous `gix` view
/// [`crate::native::NativeBackend`] hands out inside `spawn_blocking`,
/// while this function is the genuinely async part that shells out to
/// `git` so the push works against any smart-HTTP-capable remote.
pub async fn push(
    repo_path: &Path,
    remote_url: &str,
    refspecs: &[(String, String, bool)],
    creds: &Credentials,
) -> Result<PushReport, GitError> {
    ensure_remote(repo_path, remote_url).await?;
    let mut cmd = Command::new(git_binary());
    apply_credentials(&mut cmd, creds, Some(repo_path)).await;
    cmd.arg("-C").arg(repo_path).arg("push").arg("origin");
    for (local, remote, force) in refspecs {
        let spec = if *force {
            format!("+{local}:{remote}")
        } else {
            format!("{local}:{remote}")
        };
        cmd.arg(spec);
    }
    let output = run_git_subprocess(cmd, defaults::FETCH_PUSH_TIMEOUT, "push", remote_url).await?;
    if !output.status.success() {
        return Err(GitError::transport(
            "push",
            remote_url,
            String::from_utf8_lossy(&output.stderr).into_owned(),
        ));
    }
    let report = PushReport {
        updated: refspecs
            .iter()
            .map(|(local, remote, _)| (remote.clone(), local.clone()))
            .collect(),
        rejected: Vec::new(),
    };
    Ok(report)
}

/// Fast-forward `local_ref` to the commit `target_ref` points at.
///
/// Pure-local ref manipulation through `gix`; no network. See
/// [`crate::GitBackend::fast_forward`] for the contract.
pub fn fast_forward(
    repo: &gix::Repository,
    local_ref: &str,
    target_ref: &str,
) -> Result<FastForwardOutcome, GitError> {
    // Target must exist; this is the whole point of calling FF
    // after a fetch. Missing target is an operator error, not a
    // transient.
    let target_commit = repo
        .find_reference(target_ref)
        .map_err(|e| GitError::RevNotFound(format!("{target_ref}: {e}")))?
        .id()
        .detach();

    // Local may not exist yet (first-ever pull of a group that
    // was cloned empty). That's a legal create-from-nothing FF.
    let local_commit = repo.find_reference(local_ref).ok().map(|r| r.id().detach());

    match local_commit {
        None => {
            repo.reference(
                local_ref,
                target_commit,
                gix::refs::transaction::PreviousValue::MustNotExist,
                "mmcp: fast-forward (create)",
            )
            .map_err(ref_update_err(local_ref))?;
            Ok(FastForwardOutcome::Advanced {
                from: None,
                to: target_commit.to_string(),
            })
        }
        Some(local_id) if local_id == target_commit => Ok(FastForwardOutcome::AlreadyAt {
            commit: target_commit.to_string(),
        }),
        Some(local_id) => {
            if is_ancestor(repo, local_id, target_commit)? {
                let mut reference = repo
                    .find_reference(local_ref)
                    .map_err(ref_update_err(local_ref))?;
                reference
                    .set_target_id(target_commit, "mmcp: fast-forward")
                    .map_err(ref_update_err(local_ref))?;
                Ok(FastForwardOutcome::Advanced {
                    from: Some(local_id.to_string()),
                    to: target_commit.to_string(),
                })
            } else {
                Ok(FastForwardOutcome::NotFastForward {
                    local: local_id.to_string(),
                    target: target_commit.to_string(),
                })
            }
        }
    }
}

/// True when `ancestor` appears in the commit graph reachable from
/// `descendant`. Equality counts as ancestry, matching git's
/// `merge-base --is-ancestor`.
fn is_ancestor(
    repo: &gix::Repository,
    ancestor: gix::ObjectId,
    descendant: gix::ObjectId,
) -> Result<bool, GitError> {
    if ancestor == descendant {
        return Ok(true);
    }
    let walk = repo.rev_walk([descendant]).all().map_err(resolve_rev_err)?;
    for info in walk {
        let info = info.map_err(resolve_rev_err)?;
        if info.id == ancestor {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Verify every local ref named in the outgoing refspecs actually
/// resolves in `repo`. Turns git's opaque "src refspec does not
/// match any" into an actionable `nothing to push` error with the
/// offending ref name, which otherwise looks identical to a
/// remote-rejection and sends debuggers down the wrong path.
///
/// Empty refspec lists (used by tests exercising error paths) skip
/// the check so the subprocess surfaces its own error.
///
/// `pub(crate)`: [`crate::native::NativeBackend::push`] runs this
/// against the open `gix::Repository` handle inside its own
/// `spawn_blocking`, before awaiting the async [`push`] subprocess
/// call, which no longer holds a `gix::Repository` at all.
pub(crate) fn preflight_local_refs(
    repo: &gix::Repository,
    refspecs: &[(String, String, bool)],
) -> Result<(), GitError> {
    if refspecs.is_empty() {
        return Ok(());
    }
    for (local, _remote, _force) in refspecs {
        // Delete refspec (`:refs/heads/foo`) has an empty source
        // and is always valid.
        if local.is_empty() {
            continue;
        }
        if repo.find_reference(local.as_str()).is_err() {
            return Err(GitError::Transport {
                op: "push",
                url: repo.git_dir().to_string_lossy().into_owned(),
                stderr: format!(
                    "nothing to push: local ref `{local}` does not exist \
                     (repository has no commits yet, or the branch name is wrong)"
                ),
            });
        }
    }
    Ok(())
}

/// Ensure the repo at `repo_path` has an `origin` remote pointing
/// at `remote_url`. Safe to call repeatedly.
///
/// Purely local: `git remote set-url`/`git remote add` only read and
/// rewrite `repo_path`'s own config file, no network I/O. Still
/// `async fn` built on `tokio::process::Command`, bounded by
/// [`defaults::LOCAL_GIT_OP_TIMEOUT`], so this never blocks the
/// calling task's worker thread even briefly: [`fetch`] and [`push`]
/// call it directly, without wrapping it in `spawn_blocking`.
async fn ensure_remote(repo_path: &Path, remote_url: &str) -> Result<(), GitError> {
    // Try to set the URL first; if the remote does not exist,
    // fall back to adding it.
    let mut set_cmd = Command::new(git_binary());
    set_cmd
        .arg("-C")
        .arg(repo_path)
        .arg("remote")
        .arg("set-url")
        .arg("origin")
        .arg(remote_url);
    let set = run_git_subprocess(
        set_cmd,
        defaults::LOCAL_GIT_OP_TIMEOUT,
        "remote-set-url",
        remote_url,
    )
    .await?;
    if set.status.success() {
        return Ok(());
    }
    let mut add_cmd = Command::new(git_binary());
    add_cmd
        .arg("-C")
        .arg(repo_path)
        .arg("remote")
        .arg("add")
        .arg("origin")
        .arg(remote_url);
    let add = run_git_subprocess(
        add_cmd,
        defaults::LOCAL_GIT_OP_TIMEOUT,
        "remote-add",
        remote_url,
    )
    .await?;
    if !add.status.success() {
        return Err(GitError::transport(
            "remote-add",
            remote_url,
            String::from_utf8_lossy(&add.stderr).into_owned(),
        ));
    }
    Ok(())
}

/// Test-only count of [`resolve_rev`] invocations, so a unit test can
/// prove a recursive listing resolves the commit exactly once for
/// its whole descent, instead of once per directory node the way a
/// manual DFS calling [`list_tree`]/[`list_subtrees`] per node does.
/// Compiled only under `#[cfg(test)]`: no production build ever pays
/// this atomic increment.
#[cfg(test)]
pub(crate) static RESOLVE_REV_CALLS: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

/// Resolve a `Rev` to a concrete commit object id.
fn resolve_rev(repo: &gix::Repository, rev: &Rev) -> Result<gix::ObjectId, GitError> {
    #[cfg(test)]
    RESOLVE_REV_CALLS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let target = match rev {
        Rev::Branch(name) => {
            let full = format!("refs/heads/{name}");
            let reference = repo
                .find_reference(full.as_str())
                .map_err(|_| GitError::RevNotFound(full.clone()))?;
            reference.id().detach()
        }
        Rev::Tag(name) => {
            let full = format!("refs/tags/{name}");
            let reference = repo
                .find_reference(full.as_str())
                .map_err(|_| GitError::RevNotFound(full.clone()))?;
            reference.id().detach()
        }
        Rev::Commit(hex) => gix::ObjectId::from_hex(hex.as_bytes())
            .map_err(|_| GitError::RevNotFound(hex.clone()))?,
        Rev::Head => resolve_head(repo)?,
    };
    Ok(target)
}

/// Resolve `HEAD` to a commit, tolerating a branch-name mismatch.
///
/// A repo's HEAD can point at an unborn or missing branch: it was
/// init'd with `init.defaultBranch=master` while mmcp committed to
/// `main`, or it was cloned from a `master` remote. Fall back to the
/// `main` then `master` branch so reads never break on the
/// default-name mismatch.
fn resolve_head(repo: &gix::Repository) -> Result<gix::ObjectId, GitError> {
    if let Ok(id) = repo.head_id() {
        return Ok(id.detach());
    }
    let candidates = [
        format!("refs/heads/{}", mmcp_core::conventions::MAIN_BRANCH),
        "refs/heads/master".to_string(),
    ];
    for name in candidates {
        if let Ok(reference) = repo.find_reference(name.as_str()) {
            return Ok(reference.id().detach());
        }
    }
    Err(GitError::RevNotFound("HEAD".to_string()))
}

/// Resolve `rev` to its tip commit and return that commit's metadata,
/// without walking history.
///
/// Callers that only need the tip commit (for example: "does this
/// group have any commits at all") use this instead of
/// [`walk_history`], which additionally rev-walks the whole ancestry
/// and diffs every commit's blob at a path.
pub fn tip_commit(repo: &gix::Repository, rev: &Rev) -> Result<CommitMeta, GitError> {
    let commit_id = resolve_rev(repo, rev)?;
    let commit_obj = repo.find_object(commit_id).map_err(resolve_rev_err)?;
    let decoded_commit: gix::objs::Commit = commit_obj
        .into_commit()
        .decode()
        .map_err(resolve_rev_err)?
        .into_owned()
        .map_err(resolve_rev_err)?;
    Ok(commit_meta(commit_id, &decoded_commit))
}

/// Build a [`CommitMeta`] from a decoded commit object and its id.
fn commit_meta(id: gix::ObjectId, commit: &gix::objs::Commit) -> CommitMeta {
    let message_str = commit.message.to_string();
    let subject = message_str.lines().next().unwrap_or("").to_string();
    CommitMeta {
        id: id.to_string(),
        subject,
        message: message_str,
        author_name: commit.author.name.to_string(),
        author_email: commit.author.email.to_string(),
        timestamp: commit.author.time.seconds,
    }
}

/// Walk `path` inside `tree`, returning the object id of the blob if
/// any. Handles nested directories separated by `/`.
fn find_blob_in_tree(
    repo: &gix::Repository,
    root_tree_id: gix::ObjectId,
    path: &str,
) -> Result<Option<gix::ObjectId>, GitError> {
    let components: Vec<&str> = path.split('/').filter(|c| !c.is_empty()).collect();
    if components.is_empty() {
        return Ok(None);
    }
    let mut current_tree_id = root_tree_id;
    for (idx, name) in components.iter().enumerate() {
        let tree_obj = repo.find_object(current_tree_id).map_err(read_blob_err)?;
        let tree: gix::objs::Tree = tree_obj.into_tree().decode().map_err(read_blob_err)?.into();
        let name_bytes = name.as_bytes();
        let entry = tree
            .entries
            .iter()
            .find(|e| AsRef::<[u8]>::as_ref(&e.filename) == name_bytes);
        match entry {
            None => return Ok(None),
            Some(entry) => {
                let is_last = idx == components.len() - 1;
                match entry.mode.kind() {
                    EntryKind::Blob | EntryKind::BlobExecutable if is_last => {
                        return Ok(Some(entry.oid));
                    }
                    EntryKind::Tree if !is_last => {
                        current_tree_id = entry.oid;
                    }
                    _ => return Ok(None),
                }
            }
        }
    }
    Ok(None)
}

/// List the blob names directly under `path_prefix` at the given
/// revision.
///
/// Empty prefix means the root tree. Missing prefix returns an
/// empty vector rather than an error.
pub fn list_tree(
    repo: &gix::Repository,
    path_prefix: &str,
    rev: &Rev,
) -> Result<Vec<String>, GitError> {
    let commit_id = match resolve_rev(repo, rev) {
        Ok(id) => id,
        // A brand-new repo with no `main` branch yet has nothing to
        // list; that is an empty tree, not an error.
        Err(GitError::RevNotFound(_)) => return Ok(Vec::new()),
        Err(other) => return Err(other),
    };
    let commit_obj = repo.find_object(commit_id).map_err(resolve_rev_err)?;
    let commit: gix::objs::Commit = commit_obj
        .into_commit()
        .decode()
        .map_err(resolve_rev_err)?
        .into_owned()
        .map_err(resolve_rev_err)?;

    // Walk from the commit's root tree down into `path_prefix`.
    let target_tree_id = match resolve_tree_prefix(repo, commit.tree, path_prefix)? {
        Some(id) => id,
        None => return Ok(Vec::new()),
    };

    let obj = repo.find_object(target_tree_id).map_err(read_blob_err)?;
    let tree: gix::objs::Tree = obj.into_tree().decode().map_err(read_blob_err)?.into();
    let mut out = Vec::new();
    for entry in tree.entries {
        if matches!(
            entry.mode.kind(),
            EntryKind::Blob | EntryKind::BlobExecutable
        ) {
            let name = String::from_utf8_lossy(&entry.filename).into_owned();
            out.push(name);
        }
    }
    out.sort();
    Ok(out)
}

/// List the subtree names directly under `path_prefix` at the
/// given revision. Mirror of [`list_tree`] but filtered to
/// directory entries instead of blobs.
pub fn list_subtrees(
    repo: &gix::Repository,
    path_prefix: &str,
    rev: &Rev,
) -> Result<Vec<String>, GitError> {
    let commit_id = match resolve_rev(repo, rev) {
        Ok(id) => id,
        Err(GitError::RevNotFound(_)) => return Ok(Vec::new()),
        Err(other) => return Err(other),
    };
    let commit_obj = repo.find_object(commit_id).map_err(resolve_rev_err)?;
    let commit: gix::objs::Commit = commit_obj
        .into_commit()
        .decode()
        .map_err(resolve_rev_err)?
        .into_owned()
        .map_err(resolve_rev_err)?;

    let target_tree_id = match resolve_tree_prefix(repo, commit.tree, path_prefix)? {
        Some(id) => id,
        None => return Ok(Vec::new()),
    };

    let obj = repo.find_object(target_tree_id).map_err(read_blob_err)?;
    let tree: gix::objs::Tree = obj.into_tree().decode().map_err(read_blob_err)?.into();
    let mut out = Vec::new();
    for entry in tree.entries {
        if entry.mode.kind() == EntryKind::Tree {
            let name = String::from_utf8_lossy(&entry.filename).into_owned();
            out.push(name);
        }
    }
    out.sort();
    Ok(out)
}

/// One directory node reached while recursively listing a subtree:
/// its path relative to the listed prefix (empty for the prefix
/// itself), paired with the blob (file) names held directly inside
/// it. Every directory reached appears here, whether or not it
/// holds a blob directly, since a caller doing its own leaf test
/// still needs to see the empty ones.
pub type TreeDirEntry = (String, Vec<String>);

/// Recursively list every directory reached while descending from
/// `path_prefix` at the given revision, in a single tree traversal.
///
/// Resolves the commit and the target tree once, then decodes the
/// whole subtree in one pass. A caller that needs a full recursive
/// listing (for example a DFS over slug directories) uses this
/// instead of calling [`list_tree`] and [`list_subtrees`] once per
/// directory node, which each re-resolve the commit and root tree
/// from scratch.
///
/// Missing prefix or an unborn `main` branch returns an empty
/// vector, mirroring [`list_tree`] and [`list_subtrees`].
pub fn list_tree_recursive(
    repo: &gix::Repository,
    path_prefix: &str,
    rev: &Rev,
) -> Result<Vec<TreeDirEntry>, GitError> {
    let commit_id = match resolve_rev(repo, rev) {
        Ok(id) => id,
        Err(GitError::RevNotFound(_)) => return Ok(Vec::new()),
        Err(other) => return Err(other),
    };
    let commit_obj = repo.find_object(commit_id).map_err(resolve_rev_err)?;
    let commit: gix::objs::Commit = commit_obj
        .into_commit()
        .decode()
        .map_err(resolve_rev_err)?
        .into_owned()
        .map_err(resolve_rev_err)?;

    let target_tree_id = match resolve_tree_prefix(repo, commit.tree, path_prefix)? {
        Some(id) => id,
        None => return Ok(Vec::new()),
    };

    let tree_obj = repo.find_object(target_tree_id).map_err(read_blob_err)?;
    let entries = tree_obj
        .into_tree()
        .traverse()
        .breadthfirst
        .files()
        .map_err(read_blob_err)?;

    let mut dirs: std::collections::HashMap<String, Vec<String>> = std::collections::HashMap::new();
    // The prefix itself always appears, even when it holds no blobs
    // and no subtrees, so a caller sees it was reached.
    dirs.entry(String::new()).or_default();
    for entry in entries {
        let path = String::from_utf8_lossy(&entry.filepath).into_owned();
        match entry.mode.kind() {
            EntryKind::Tree => {
                dirs.entry(path).or_default();
            }
            EntryKind::Blob | EntryKind::BlobExecutable => {
                let (dir, name) = match path.rsplit_once('/') {
                    Some((dir, name)) => (dir.to_string(), name.to_string()),
                    None => (String::new(), path),
                };
                dirs.entry(dir).or_default().push(name);
            }
            _ => {}
        }
    }
    let mut out: Vec<TreeDirEntry> = dirs.into_iter().collect();
    for (_, filenames) in &mut out {
        filenames.sort();
    }
    Ok(out)
}

/// Walk from `root_tree_id` into the subtree at `path_prefix`,
/// returning its tree id or `None` if the prefix does not resolve
/// to an existing subtree.
fn resolve_tree_prefix(
    repo: &gix::Repository,
    root_tree_id: gix::ObjectId,
    path_prefix: &str,
) -> Result<Option<gix::ObjectId>, GitError> {
    let components: Vec<&str> = path_prefix.split('/').filter(|c| !c.is_empty()).collect();
    if components.is_empty() {
        return Ok(Some(root_tree_id));
    }
    let mut current = root_tree_id;
    for name in components {
        let obj = repo.find_object(current).map_err(read_blob_err)?;
        let tree: gix::objs::Tree = obj.into_tree().decode().map_err(read_blob_err)?.into();
        let name_bytes = name.as_bytes();
        let entry = tree
            .entries
            .iter()
            .find(|e| AsRef::<[u8]>::as_ref(&e.filename) == name_bytes);
        match entry {
            None => return Ok(None),
            Some(entry) if entry.mode.kind() == EntryKind::Tree => {
                current = entry.oid;
            }
            Some(_) => return Ok(None),
        }
    }
    Ok(Some(current))
}

/// Read the blob at `path` inside `tree_id`.
fn read_blob_at_tree(
    repo: &gix::Repository,
    tree_id: gix::ObjectId,
    path: &str,
) -> Result<Bytes, GitError> {
    let blob_id = find_blob_in_tree(repo, tree_id, path)?
        .ok_or_else(|| GitError::PathNotFound(path.to_string()))?;
    let blob = repo.find_object(blob_id).map_err(read_blob_err)?;
    Ok(Bytes::from(blob.data.clone()))
}

/// Read the contents of `path` inside the commit at `rev`.
pub fn read_file(repo: &gix::Repository, path: &str, rev: &Rev) -> Result<Bytes, GitError> {
    let commit_id = resolve_rev(repo, rev)?;
    let commit_obj = repo.find_object(commit_id).map_err(resolve_rev_err)?;
    let commit: gix::objs::Commit = commit_obj
        .into_commit()
        .decode()
        .map_err(resolve_rev_err)?
        .into_owned()
        .map_err(resolve_rev_err)?;
    read_blob_at_tree(repo, commit.tree, path)
}

/// Per-path outcome of a [`read_files`] batch: the requested path
/// paired with its read result, so one missing or unreadable file
/// never aborts the whole batch.
pub type BatchReadResult = Vec<(String, Result<Bytes, GitError>)>;

/// Read the contents of every path in `paths` at the same revision.
///
/// The commit and its root tree are resolved once and reused for
/// every lookup, instead of paying the resolve cost per file like
/// calling [`read_file`] in a loop would. Each path keeps its own
/// outcome, in request order, so one missing or unreadable path
/// never aborts the batch.
pub fn read_files(
    repo: &gix::Repository,
    paths: &[String],
    rev: &Rev,
) -> Result<BatchReadResult, GitError> {
    let commit_id = resolve_rev(repo, rev)?;
    let commit_obj = repo.find_object(commit_id).map_err(resolve_rev_err)?;
    let commit: gix::objs::Commit = commit_obj
        .into_commit()
        .decode()
        .map_err(resolve_rev_err)?
        .into_owned()
        .map_err(resolve_rev_err)?;

    Ok(paths
        .iter()
        .map(|path| {
            let outcome = read_blob_at_tree(repo, commit.tree, path);
            (path.clone(), outcome)
        })
        .collect())
}

/// A path pre-split into non-empty components.
///
/// Normalizes leading, trailing, and repeated `/` separators the
/// same way the old manual tree walk did, so a path like `a//b/`
/// still resolves to the two components `a`, `b` instead of
/// tripping the tree editor's rejection of empty path components.
struct PathComponents<'a>(Vec<&'a str>);

impl<'a> PathComponents<'a> {
    fn split(path: &'a str) -> Self {
        PathComponents(path.split('/').filter(|c| !c.is_empty()).collect())
    }
}

impl ToComponents for PathComponents<'_> {
    fn to_components(&self) -> impl Iterator<Item = &BStr> {
        self.0.iter().map(|c| BStr::new(c.as_bytes()))
    }
}

/// Borrowed view over a prefix of a [`PathComponents`] list, for
/// probing one intermediate directory at a time.
struct ComponentsRef<'a>(&'a [&'a str]);

impl ToComponents for ComponentsRef<'_> {
    fn to_components(&self) -> impl Iterator<Item = &BStr> {
        self.0.iter().map(|c| BStr::new(c.as_bytes()))
    }
}

/// Reject a path whose intermediate directory component already
/// exists as a non-tree entry (a prior blob committed at that exact
/// path), either freshly edited earlier in this same batch or
/// already committed to `root_id`'s history. The tree editor itself
/// would silently coerce the blob into a tree instead of treating
/// this as the invariant violation it is, so the check is performed
/// explicitly before editing.
///
/// `editor.get()` only sees trees already materialized by an
/// earlier edit in this batch, so a component it hasn't loaded yet
/// falls back to looking up the ORIGINAL tree at `root_id`: without
/// that fallback a blob committed in a prior, separate commit is
/// never seen by this guard at all.
fn reject_blob_directory_collision(
    repo: &gix::Repository,
    editor: &gix::object::tree::Editor<'_>,
    root_id: gix::ObjectId,
    path: &str,
    components: &[&str],
) -> Result<(), GitError> {
    for depth in 1..components.len() {
        let prefix = &components[..depth];
        let is_tree = match editor.get(ComponentsRef(prefix)) {
            Some(entry) => entry.mode().is_tree(),
            None => {
                let root = repo.find_tree(root_id).map_err(commit_err)?;
                match root
                    .lookup_entry(prefix.iter().copied())
                    .map_err(commit_err)?
                {
                    Some(entry) => entry.mode().is_tree(),
                    // Absent on disk too: this edit will create it
                    // as a fresh tree, which is not a collision.
                    None => true,
                }
            }
        };
        if !is_tree {
            return Err(commit_msg_err(format!(
                "path component collides with an existing blob: {path}"
            )));
        }
    }
    Ok(())
}

/// Construct a new tree by applying the edits in `files` on top of
/// the parent tree. Returns the new tree object id.
///
/// Uses `gix`'s sparse tree editor instead of a full recursive
/// rebuild: it loads and rewrites only the trees on each edited
/// path, so a sibling subtree untouched by any edit keeps its
/// original object id and is never re-read, re-serialized, or
/// rewritten to the object database.
fn build_tree(
    repo: &gix::Repository,
    parent_tree_id: Option<gix::ObjectId>,
    files: &[(String, Option<Vec<u8>>)],
) -> Result<gix::ObjectId, GitError> {
    let root_id = parent_tree_id.unwrap_or_else(|| gix::ObjectId::empty_tree(repo.object_hash()));
    let mut editor = repo.edit_tree(root_id).map_err(commit_err)?;

    for (path, contents) in files {
        let components = PathComponents::split(path);
        if components.0.is_empty() {
            return Err(GitError::PathNotFound(path.clone()));
        }
        reject_blob_directory_collision(repo, &editor, root_id, path, &components.0)?;
        match contents {
            Some(bytes) => {
                let blob_id = repo
                    .write_blob(bytes.as_slice())
                    .map_err(commit_err)?
                    .detach();
                editor
                    .upsert(components, EntryKind::Blob, blob_id)
                    .map_err(commit_err)?;
            }
            None => {
                editor.remove(components).map_err(commit_err)?;
            }
        }
    }

    Ok(editor.write().map_err(commit_err)?.detach())
}

/// Create a new commit on the given branch applying a set of file
/// edits. The branch is created if it does not yet exist.
pub fn write_commit(repo: &gix::Repository, spec: CommitSpec) -> Result<String, GitError> {
    let branch_ref = format!("refs/heads/{}", spec.branch);

    let (parent_commit_id, parent_tree_id) = match repo.find_reference(branch_ref.as_str()) {
        Ok(reference) => {
            let parent_commit_id = reference.id().detach();
            let commit_obj = repo.find_object(parent_commit_id).map_err(commit_err)?;
            let commit: gix::objs::Commit = commit_obj
                .into_commit()
                .decode()
                .map_err(commit_err)?
                .into_owned()
                .map_err(commit_err)?;
            (Some(parent_commit_id), Some(commit.tree))
        }
        Err(_) => (None, None),
    };

    let new_tree_id = build_tree(repo, parent_tree_id, &spec.files)?;

    let now = gix::date::Time::now_local_or_utc();
    let signature = gix::actor::Signature {
        name: BString::from(spec.author_name.as_str()),
        email: BString::from(spec.author_email.as_str()),
        time: now,
    };
    let commit = gix::objs::Commit {
        tree: new_tree_id,
        parents: parent_commit_id.into_iter().collect(),
        author: signature.clone(),
        committer: signature,
        encoding: None,
        message: BString::from(spec.message.as_str()),
        extra_headers: Vec::new(),
    };
    let commit_id = repo.write_object(&commit).map_err(commit_err)?.detach();

    let log_message = "mmcp: write commit";
    match repo.find_reference(branch_ref.as_str()) {
        Ok(mut reference) => {
            reference
                .set_target_id(commit_id, log_message)
                .map_err(ref_update_err(branch_ref.as_str()))?;
        }
        Err(_) => {
            repo.reference(
                branch_ref.as_str(),
                commit_id,
                gix::refs::transaction::PreviousValue::MustNotExist,
                log_message,
            )
            .map_err(ref_update_err(branch_ref.as_str()))?;
        }
    }

    Ok(commit_id.to_string())
}

/// Create a lightweight tag pointing at `target_hex`.
pub fn tag(repo: &gix::Repository, name: &str, target_hex: &str) -> Result<(), GitError> {
    let target = gix::ObjectId::from_hex(target_hex.as_bytes())
        .map_err(|_| GitError::RevNotFound(target_hex.to_string()))?;
    let refname = format!("refs/tags/{name}");
    repo.reference(
        refname.as_str(),
        target,
        gix::refs::transaction::PreviousValue::Any,
        "mmcp: tag",
    )
    .map_err(ref_update_err(refname.as_str()))?;
    Ok(())
}

/// Walk the commit history that modified `path`, most recent first.
///
/// Matches `git log -- <path>` semantics: a commit is included only
/// when its blob at `path` differs from *every* parent's blob at
/// the same path (or the path didn't exist in any parent). Commits
/// that merely carry the file forward unchanged from a parent are
/// filtered out: otherwise every commit since the file was
/// introduced would appear, which is how GUIs get surprising
/// "100 commits" counts on a memory that was only edited twice.
///
/// Reads start from whatever `HEAD` points at, so the walker works
/// against repos whose default branch is not `main` (cloned from
/// `master`-based forges, custom-named defaults, etc.).
pub fn walk_history(repo: &gix::Repository, path: &str) -> Result<Vec<CommitMeta>, GitError> {
    let head = match resolve_rev(repo, &Rev::Head) {
        Ok(id) => id,
        Err(GitError::RevNotFound(_)) => return Ok(Vec::new()),
        Err(other) => return Err(other),
    };

    let mut out = Vec::new();
    let walk = repo.rev_walk([head]).all().map_err(resolve_rev_err)?;
    for info in walk {
        let info = info.map_err(resolve_rev_err)?;
        let commit_obj = repo.find_object(info.id).map_err(resolve_rev_err)?;
        let decoded_commit: gix::objs::Commit = commit_obj
            .into_commit()
            .decode()
            .map_err(resolve_rev_err)?
            .into_owned()
            .map_err(resolve_rev_err)?;

        // No blob at `path` → this commit can't be a modification of it.
        let Some(current_blob) = find_blob_in_tree(repo, decoded_commit.tree, path)? else {
            continue;
        };

        // If any parent already had the exact same blob at the same
        // path, the file was carried forward unchanged and this
        // commit is not a modification of it.
        let mut matches_parent = false;
        for parent_id in &decoded_commit.parents {
            let parent_obj = repo.find_object(*parent_id).map_err(resolve_rev_err)?;
            let parent_commit: gix::objs::Commit = parent_obj
                .into_commit()
                .decode()
                .map_err(resolve_rev_err)?
                .into_owned()
                .map_err(resolve_rev_err)?;
            if let Some(parent_blob) = find_blob_in_tree(repo, parent_commit.tree, path)?
                && parent_blob == current_blob
            {
                matches_parent = true;
                break;
            }
        }
        if matches_parent {
            continue;
        }

        out.push(commit_meta(info.id, &decoded_commit));
    }
    Ok(out)
}

#[cfg(test)]
mod credential_tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]
    use super::*;
    use tempfile::TempDir;

    /// Collect every `(key, value)` env var `apply_credentials` set on
    /// `cmd`, so a test can assert presence/absence and value without
    /// depending on `std::process::Command`'s (partial) `Debug` output.
    /// Goes through `as_std()`: `tokio::process::Command` wraps a
    /// `std::process::Command` internally and exposes its own
    /// inspection methods only via that accessor.
    fn envs_of(cmd: &Command) -> std::collections::HashMap<String, String> {
        cmd.as_std()
            .get_envs()
            .filter_map(|(k, v)| {
                Some((
                    k.to_string_lossy().into_owned(),
                    v?.to_string_lossy().into_owned(),
                ))
            })
            .collect()
    }

    /// Collect every argv token `apply_credentials` appended to `cmd`.
    fn args_of(cmd: &Command) -> Vec<String> {
        cmd.as_std()
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    /// A repo path guaranteed not to exist on disk. `git -C <path>
    /// config --get` fails to change directory and returns immediately,
    /// before it would otherwise fall through to reading the host
    /// machine's real global/system `core.sshCommand`. Every test in
    /// this module that exercises `apply_credentials`/
    /// `suppress_interactive_prompts` without itself caring about tier
    /// 3's `core.sshCommand` resolution passes this, so those tests
    /// stay hermetic and deterministic regardless of what the host
    /// machine's own git config happens to contain.
    fn nonexistent_repo_path() -> std::path::PathBuf {
        std::env::temp_dir().join("mmcp-git-nonexistent-repo-for-ssh-command-tests")
    }

    /// Falsification: `Credentials::None` must still suppress every
    /// interactive fallback, including a default headless
    /// `GIT_SSH_COMMAND`, even though it sets no explicit credential of
    /// its own. An mmcp caller has no human present to answer a prompt,
    /// so the ambient-environment default must still fail fast.
    #[tokio::test]
    async fn credentials_none_still_suppresses_interactive_prompts() {
        let mut cmd = Command::new("git");
        apply_credentials(&mut cmd, &Credentials::None, Some(&nonexistent_repo_path())).await;

        let args = args_of(&cmd);
        assert!(
            args.windows(2).any(|w| w == ["-c", "credential.helper="]),
            "Credentials::None must disable credential.helper via -c, got: {args:?}"
        );
        let envs = envs_of(&cmd);
        assert_eq!(
            envs.get("GIT_TERMINAL_PROMPT").map(String::as_str),
            Some("0")
        );
        assert_eq!(envs.get("GIT_ASKPASS").map(String::as_str), Some(""));
        assert!(
            !envs.contains_key("GIT_CONFIG_COUNT"),
            "Credentials::None must not set any bearer config env"
        );
        let ssh_command = envs.get("GIT_SSH_COMMAND").map(String::as_str);
        assert!(
            ssh_command.is_some_and(|v| v.contains("BatchMode=yes")),
            "Credentials::None must default GIT_SSH_COMMAND to a BatchMode=yes ssh \
             invocation, got: {ssh_command:?}"
        );
    }

    /// Falsification: `Credentials::BearerHttp` must both apply the
    /// bearer header AND suppress every interactive fallback, so an
    /// automated caller with a definitive (if possibly stale/invalid)
    /// token never blocks on a GCM/terminal/askpass prompt.
    #[tokio::test]
    async fn credentials_bearer_http_suppresses_interactive_prompts() {
        let mut cmd = Command::new("git");
        apply_credentials(
            &mut cmd,
            &Credentials::BearerHttp("s3cr3t".to_string()),
            Some(&nonexistent_repo_path()),
        )
        .await;

        let args = args_of(&cmd);
        assert!(
            args.windows(2).any(|w| w == ["-c", "credential.helper="]),
            "BearerHttp must disable credential.helper via -c, got: {args:?}"
        );
        let envs = envs_of(&cmd);
        assert_eq!(
            envs.get("GIT_TERMINAL_PROMPT").map(String::as_str),
            Some("0")
        );
        assert_eq!(envs.get("GIT_ASKPASS").map(String::as_str), Some(""));
        assert_eq!(
            envs.get("GIT_CONFIG_VALUE_0").map(String::as_str),
            Some("Authorization: Bearer s3cr3t"),
            "the bearer header itself must still be applied"
        );
        let ssh_command = envs.get("GIT_SSH_COMMAND").map(String::as_str);
        assert!(
            ssh_command.is_some_and(|v| v.contains("BatchMode=yes")),
            "BearerHttp must default GIT_SSH_COMMAND to a BatchMode=yes ssh invocation, \
             got: {ssh_command:?}"
        );
    }

    /// Falsification: `Credentials::SshCommand` must also suppress
    /// interactive fallbacks, for the same automated-caller reasoning
    /// as `BearerHttp`.
    #[tokio::test]
    async fn credentials_ssh_command_suppresses_interactive_prompts() {
        let mut cmd = Command::new("git");
        apply_credentials(
            &mut cmd,
            &Credentials::SshCommand("ssh -i /key".to_string()),
            Some(&nonexistent_repo_path()),
        )
        .await;

        let args = args_of(&cmd);
        assert!(
            args.windows(2).any(|w| w == ["-c", "credential.helper="]),
            "SshCommand must disable credential.helper via -c, got: {args:?}"
        );
        let envs = envs_of(&cmd);
        assert_eq!(
            envs.get("GIT_TERMINAL_PROMPT").map(String::as_str),
            Some("0")
        );
        assert_eq!(envs.get("GIT_ASKPASS").map(String::as_str), Some(""));
        assert_eq!(
            envs.get("GIT_SSH_COMMAND").map(String::as_str),
            Some("ssh -i /key"),
            "the ssh command itself must still be applied"
        );
    }

    /// Falsification: the default-`GIT_SSH_COMMAND` injection in
    /// [`suppress_interactive_prompts`] must never overwrite a caller-
    /// supplied `Credentials::SshCommand` value, even though that value
    /// carries no `BatchMode=yes` flag of its own. The variant is the
    /// caller's explicit escape hatch and stays fully caller-controlled.
    #[tokio::test]
    async fn credentials_ssh_command_value_is_not_overwritten_by_the_batch_mode_default() {
        let mut cmd = Command::new("git");
        apply_credentials(
            &mut cmd,
            &Credentials::SshCommand("ssh -i /custom/key -o IdentitiesOnly=yes".to_string()),
            Some(&nonexistent_repo_path()),
        )
        .await;

        let envs = envs_of(&cmd);
        assert_eq!(
            envs.get("GIT_SSH_COMMAND").map(String::as_str),
            Some("ssh -i /custom/key -o IdentitiesOnly=yes"),
            "a caller-supplied GIT_SSH_COMMAND must survive verbatim, with no \
             BatchMode=yes flag injected on top of it"
        );
    }

    /// Falsification: `Credentials::None` end to end, through
    /// `apply_credentials`, still lands a `GIT_SSH_COMMAND` containing
    /// `BatchMode=yes` on the `Command` builder regardless of whatever
    /// ambient value the real test-process environment happens to
    /// carry, since `resolve_ssh_command` always appends or defaults to
    /// a `BatchMode=yes`-bearing value.
    #[tokio::test]
    async fn credentials_none_ssh_command_always_contains_batch_mode() {
        let mut cmd = Command::new("git");
        apply_credentials(&mut cmd, &Credentials::None, Some(&nonexistent_repo_path())).await;

        let envs = envs_of(&cmd);
        let ssh_command = envs.get("GIT_SSH_COMMAND").map(String::as_str);
        assert!(
            ssh_command.is_some_and(|v| v.contains("BatchMode=yes")),
            "Credentials::None must always land a BatchMode=yes GIT_SSH_COMMAND, \
             got: {ssh_command:?}"
        );
    }

    /// Falsification: an ambient `GIT_SSH_COMMAND` (simulated as the
    /// `Some` input `resolve_ssh_command` receives, standing in for a
    /// real `std::env::var_os` read) must be preserved and extended
    /// with the batch-mode flag, never replaced outright, so an
    /// operator's own custom identity/tool keeps working. This crate
    /// forbids `unsafe` code, so the resolution logic is a pure
    /// function taking the ambient value as a parameter rather than a
    /// test that mutates the real process environment via
    /// `std::env::set_var` (which requires `unsafe`).
    #[test]
    fn resolve_ssh_command_ambient_wins_and_is_extended() {
        let ambient = OsString::from("ssh -i /custom/ambient/key -o IdentitiesOnly=yes");

        let resolved = resolve_ssh_command(Some(ambient), None);

        assert_eq!(
            resolved.to_str(),
            Some("ssh -i /custom/ambient/key -o IdentitiesOnly=yes -o BatchMode=yes"),
            "an ambient GIT_SSH_COMMAND must be preserved and extended, never replaced"
        );
    }

    /// Falsification: git's own real precedence between the
    /// `GIT_SSH_COMMAND` environment variable and the `core.sshCommand`
    /// config key must hold here too: an ambient value present
    /// alongside a `core.sshCommand` value wins outright, the config
    /// value is never even consulted.
    #[test]
    fn resolve_ssh_command_ambient_wins_over_core_ssh_command() {
        let ambient = OsString::from("ssh -i /ambient/key");
        let core_ssh_command = OsString::from("C:/Windows/System32/OpenSSH/ssh.exe");

        let resolved = resolve_ssh_command(Some(ambient), Some(core_ssh_command));

        assert_eq!(
            resolved.to_str(),
            Some("ssh -i /ambient/key -o BatchMode=yes"),
            "an ambient GIT_SSH_COMMAND must win over a core.sshCommand value"
        );
    }

    /// Falsification: with no ambient value, a `core.sshCommand` value
    /// is picked up and extended with the batch-mode flag exactly like
    /// an ambient value would be, so a working `core.sshCommand` is
    /// honored rather than silently overridden by the bare default.
    #[test]
    fn resolve_ssh_command_uses_core_ssh_command_when_ambient_absent() {
        let core_ssh_command = OsString::from("C:/Windows/System32/OpenSSH/ssh.exe");

        let resolved = resolve_ssh_command(None, Some(core_ssh_command));

        assert_eq!(
            resolved.to_str(),
            Some("C:/Windows/System32/OpenSSH/ssh.exe -o BatchMode=yes"),
            "core.sshCommand must be picked up and extended when no ambient value exists"
        );
    }

    /// Falsification: an ambient value that is set but empty (a
    /// distinct case from unset) must fall through to `core.sshCommand`
    /// exactly like an absent ambient value, not extended into a
    /// leading-space command of its own.
    #[test]
    fn resolve_ssh_command_uses_core_ssh_command_when_ambient_empty() {
        let core_ssh_command = OsString::from("C:/Windows/System32/OpenSSH/ssh.exe");

        let resolved = resolve_ssh_command(Some(OsString::new()), Some(core_ssh_command));

        assert_eq!(
            resolved.to_str(),
            Some("C:/Windows/System32/OpenSSH/ssh.exe -o BatchMode=yes"),
            "an empty ambient value must fall through to core.sshCommand, not block it"
        );
    }

    /// Falsification: with neither an ambient value nor a
    /// `core.sshCommand` value, the bare default from
    /// [`defaults::DEFAULT_SSH_COMMAND_BATCH_MODE`] applies verbatim.
    #[test]
    fn resolve_ssh_command_defaults_with_neither_tier_set() {
        let resolved = resolve_ssh_command(None, None);

        assert_eq!(
            resolved.to_str(),
            Some(defaults::DEFAULT_SSH_COMMAND_BATCH_MODE),
            "with neither tier set, the bare default must apply verbatim"
        );
    }

    /// Falsification: an ambient value and a `core.sshCommand` value
    /// that are both set but empty must be treated the same as neither
    /// being set at all.
    #[test]
    fn resolve_ssh_command_defaults_when_both_tiers_empty() {
        let resolved = resolve_ssh_command(Some(OsString::new()), Some(OsString::new()));

        assert_eq!(
            resolved.to_str(),
            Some(defaults::DEFAULT_SSH_COMMAND_BATCH_MODE),
            "two empty values must fall back to the bare default, not extend nothing"
        );
    }

    /// Falsification: [`build_core_ssh_command_query`] scopes to the
    /// given repo via `-C`, with no `--global` flag, for the
    /// `fetch`/`push` case where a repo already exists.
    #[test]
    fn build_core_ssh_command_query_scopes_to_repo_for_fetch_and_push() {
        let cmd = build_core_ssh_command_query(Some(Path::new("/some/repo")));

        let args = args_of(&cmd);
        assert_eq!(
            args,
            vec!["-C", "/some/repo", "config", "--get", "core.sshCommand"],
            "a repo path must scope the lookup via -C, with no --global flag"
        );
    }

    /// Falsification: [`build_core_ssh_command_query`] scopes to
    /// `--global` only, with no `-C`, for the `clone` case where no
    /// repo exists yet to scope a local lookup to.
    #[test]
    fn build_core_ssh_command_query_scopes_to_global_for_clone() {
        let cmd = build_core_ssh_command_query(None);

        let args = args_of(&cmd);
        assert_eq!(
            args,
            vec!["config", "--get", "--global", "core.sshCommand"],
            "no repo path must scope the lookup to --global only, with no -C"
        );
    }

    /// Falsification: a successful exit with real stdout content yields
    /// the trimmed value.
    #[test]
    fn parse_core_ssh_command_output_returns_value_on_success() {
        let resolved = parse_core_ssh_command_output(true, b"ssh -i /key\n");

        assert_eq!(resolved, Some(OsString::from("ssh -i /key")));
    }

    /// Falsification: a failed exit (git's own exit code `1` for an
    /// unset key, or any other nonzero code) yields `None`, never the
    /// raw stdout content.
    #[test]
    fn parse_core_ssh_command_output_none_on_failure() {
        let resolved = parse_core_ssh_command_output(false, b"");

        assert_eq!(resolved, None);
    }

    /// Falsification: a successful exit with empty stdout (git prints
    /// nothing when the effective value itself is an empty string)
    /// yields `None`, not `Some("")`.
    #[test]
    fn parse_core_ssh_command_output_none_on_empty_success() {
        let resolved = parse_core_ssh_command_output(true, b"\n");

        assert_eq!(resolved, None);
    }

    /// Write `core.sshCommand = value` to `repo_path`'s own local git
    /// config, via a real `git config` subprocess, so
    /// [`read_core_ssh_command`] tests below exercise the real
    /// resolution path rather than a mocked stand-in.
    async fn set_local_core_ssh_command(repo_path: &Path, value: &str) {
        let mut cmd = Command::new(git_binary());
        cmd.arg("-C")
            .arg(repo_path)
            .arg("config")
            .arg("core.sshCommand")
            .arg(value);
        let output = run_git_subprocess(
            cmd,
            defaults::LOCAL_GIT_OP_TIMEOUT,
            "config-set",
            "core.sshCommand",
        )
        .await
        .expect("git config core.sshCommand must spawn and complete");
        assert!(
            output.status.success(),
            "git config core.sshCommand must succeed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    /// Falsification: [`read_core_ssh_command`], given a repo whose own
    /// LOCAL config carries `core.sshCommand`, returns that value via a
    /// real `git config --get` subprocess. Uses a scratch bare repo's
    /// own local config exclusively; the host machine's real
    /// global/system `core.sshCommand` (if any) is irrelevant, since
    /// local config always wins in git's own resolution order.
    #[tokio::test]
    async fn read_core_ssh_command_reads_repo_local_config_end_to_end() {
        let tmp = TempDir::new().expect("tempdir");
        let repo_path = tmp.path().join("scratch.git");
        gix::init_bare(&repo_path).expect("init bare scratch repo");
        set_local_core_ssh_command(&repo_path, "C:/Windows/System32/OpenSSH/ssh.exe").await;

        let resolved = read_core_ssh_command(Some(&repo_path)).await;

        assert_eq!(
            resolved,
            Some(OsString::from("C:/Windows/System32/OpenSSH/ssh.exe")),
            "a repo-local core.sshCommand must be read back verbatim"
        );
    }

    /// Falsification: `apply_credentials`/`suppress_interactive_prompts`
    /// wire tier 3 end to end: with no builder-supplied credential and
    /// no ambient `GIT_SSH_COMMAND`, a scratch repo's own local
    /// `core.sshCommand` lands on the resulting `Command`'s
    /// `GIT_SSH_COMMAND`, extended with `BatchMode=yes`, instead of the
    /// bare `ssh -o BatchMode=yes` default.
    #[tokio::test]
    async fn apply_credentials_uses_repo_core_ssh_command_when_no_builder_or_ambient_value() {
        if std::env::var_os("GIT_SSH_COMMAND").is_some_and(|v| !v.is_empty()) {
            // An ambient GIT_SSH_COMMAND in this real test process's
            // environment would legitimately win over core.sshCommand
            // (tier 2 outranks tier 3): skip rather than assert a false
            // failure against a machine-specific ambient value this
            // test does not control.
            return;
        }
        let tmp = TempDir::new().expect("tempdir");
        let repo_path = tmp.path().join("scratch.git");
        gix::init_bare(&repo_path).expect("init bare scratch repo");
        set_local_core_ssh_command(&repo_path, "C:/Windows/System32/OpenSSH/ssh.exe").await;

        let mut cmd = Command::new("git");
        apply_credentials(&mut cmd, &Credentials::None, Some(&repo_path)).await;

        let envs = envs_of(&cmd);
        assert_eq!(
            envs.get("GIT_SSH_COMMAND").map(String::as_str),
            Some("C:/Windows/System32/OpenSSH/ssh.exe -o BatchMode=yes"),
            "the repo's core.sshCommand must be used instead of the bare ssh default"
        );
    }
}

#[cfg(test)]
mod timeout_tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]
    use super::*;

    /// Seconds a deliberately-hung test child sleeps for: comfortably
    /// longer than any short test timeout used against it, so the
    /// process is still alive when `wait_with_timeout` gives up and
    /// kills it, never because it happened to finish naturally.
    const HUNG_CHILD_SLEEP_SECS: u64 = 100;

    /// Spawn an OS process that sleeps far longer than any short test
    /// timeout, so the child is deterministically still running when
    /// `wait_with_timeout` gives up on it.
    ///
    /// [`wait_with_timeout`] is generic over any `tokio::process::
    /// Command`, so this does not need to be `git` itself: an
    /// OS-native sleep avoids depending on a specific subcommand's
    /// stdin-blocking behavior, which proved inconsistent across git
    /// installations.
    #[cfg(windows)]
    fn spawn_hung_child() -> Child {
        let mut cmd = Command::new("powershell");
        cmd.args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            &format!("Start-Sleep -Seconds {HUNG_CHILD_SLEEP_SECS}"),
        ]);
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());
        cmd.spawn().expect("spawn powershell Start-Sleep")
    }

    /// Unix equivalent of the Windows [`spawn_hung_child`] above.
    #[cfg(not(windows))]
    fn spawn_hung_child() -> Child {
        let mut cmd = Command::new("sleep");
        cmd.arg(HUNG_CHILD_SLEEP_SECS.to_string());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());
        cmd.spawn().expect("spawn sleep")
    }

    /// Falsification: a child that outlives `timeout` is explicitly
    /// killed and reaped, not merely reported as timed out. Red check
    /// performed manually: removing the `child.kill().await?` call
    /// from `wait_with_timeout`'s timeout branch makes this fail at
    /// the final `try_wait` assertion (`Ok(None)`, still running)
    /// instead of the `matches!` assertion, since a still-sleeping
    /// child keeps running for [`HUNG_CHILD_SLEEP_SECS`] regardless of
    /// what `wait_with_timeout` returns; restoring the call makes it
    /// pass again.
    #[tokio::test]
    async fn wait_with_timeout_kills_and_reaps_a_hung_child() {
        let mut child = spawn_hung_child();
        let short_timeout = Duration::from_millis(200);

        let result = wait_with_timeout(&mut child, short_timeout, "test-op", "test-target").await;

        assert!(
            matches!(result, Err(GitError::Timeout { op: "test-op", .. })),
            "expected GitError::Timeout, got {result:?}"
        );
        // `Child::try_wait` returns `Ok(None)` for a still-running
        // process and `Ok(Some(status))` once it has actually exited
        // and been reaped; tokio caches the exit status internally so
        // this observes the real post-kill process state, not merely
        // trusting that `wait_with_timeout` claims to have killed it.
        let post_kill_status = child
            .try_wait()
            .expect("try_wait after an explicit kill+wait must not error");
        assert!(
            post_kill_status.is_some(),
            "the child must have actually exited after the timeout branch killed it"
        );
    }

    /// Falsification: a command that finishes well inside `timeout`
    /// still returns its real exit status and output, so the timeout
    /// path above did not come at the cost of the normal case.
    #[tokio::test]
    async fn wait_with_timeout_returns_real_output_for_a_fast_command() {
        let mut cmd = Command::new(git_binary());
        cmd.arg("--version");
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());
        let mut child = cmd.spawn().expect("spawn git --version");

        let output = wait_with_timeout(
            &mut child,
            defaults::LOCAL_GIT_OP_TIMEOUT,
            "test-op",
            "test-target",
        )
        .await
        .expect("git --version must complete well inside the timeout");

        assert!(output.status.success(), "git --version must exit 0");
        assert!(
            String::from_utf8_lossy(&output.stdout).contains("git version"),
            "stdout must carry git's real version output, got: {:?}",
            String::from_utf8_lossy(&output.stdout)
        );
    }
}

#[cfg(test)]
mod stdin_tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]
    use super::*;

    /// Build a command that reads all of its own stdin to EOF and
    /// prints the byte count it saw. `run_git_subprocess` sets stdio
    /// on whatever `Command` it is handed, so the test only needs to
    /// name the program.
    #[cfg(windows)]
    fn stdin_reading_command() -> Command {
        let mut cmd = Command::new("powershell");
        cmd.args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "[Console]::In.ReadToEnd().Length",
        ]);
        cmd
    }

    /// Unix equivalent of the Windows [`stdin_reading_command`] above.
    #[cfg(not(windows))]
    fn stdin_reading_command() -> Command {
        let mut cmd = Command::new("wc");
        cmd.arg("-c");
        cmd
    }

    /// Proves `run_git_subprocess` always spawns with a null stdin,
    /// never an inherited one: `mmcp serve` is an MCP stdio server, so
    /// an inherited stdin would hand every git subprocess the live
    /// JSON-RPC request stream.
    ///
    /// There is no public getter on `std::process::Command`/
    /// `tokio::process::Command` to read a configured `Stdio` back
    /// (unlike env/args, which `credential_tests` above inspects via
    /// `Command::get_envs`/`get_args`). A black-box behavioral probe
    /// (spawn a child that reports its own stdin byte count) cannot
    /// falsify this property in this test's own CI harness: the
    /// harness's own stdin already runs closed, so a child that
    /// inherited it would also see 0 bytes, the same result a
    /// genuinely nulled stdin produces.
    ///
    /// The actual guarantee this test asserts on is structural rather
    /// than behavioral: `Command::stdin` is a plain builder setter,
    /// last call wins, so `run_git_subprocess` calling
    /// `cmd.stdin(Stdio::null())` unconditionally as the final write to
    /// that field before `cmd.spawn()` deterministically wins over
    /// anything the caller pre-configured, regardless of the ambient
    /// environment. This test pre-sets `Stdio::inherit()` explicitly
    /// (not the default; a caller could plausibly do this by mistake)
    /// to prove the override happens, then falls back to the same
    /// byte-count assertion as a secondary, environment-permitting
    /// sanity check.
    #[tokio::test]
    async fn run_git_subprocess_nulls_stdin() {
        let mut cmd = stdin_reading_command();
        cmd.stdin(Stdio::inherit());

        let output = run_git_subprocess(cmd, Duration::from_secs(5), "test-op", "test-target")
            .await
            .expect("a stdin-reading command must complete well inside the timeout");

        assert!(
            output.status.success(),
            "the stdin-reading command must exit 0"
        );
        let reported_len: usize = String::from_utf8_lossy(&output.stdout)
            .trim()
            .parse()
            .expect("stdout must be a plain byte count");
        assert_eq!(
            reported_len, 0,
            "the child must see an immediate empty stdin, consistent with \
             run_git_subprocess overriding it to Stdio::null()"
        );
    }
}

#[cfg(test)]
mod recursive_listing_tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]
    use std::sync::atomic::Ordering;

    use super::*;
    use tempfile::TempDir;

    /// Open the freshly committed scratch repo the way `NativeBackend`
    /// does: a `ThreadSafeRepository` handed out as a thread-local
    /// view.
    fn open_thread_local(repo_path: &Path) -> gix::Repository {
        gix::ThreadSafeRepository::open(repo_path)
            .expect("open scratch repo")
            .to_thread_local()
    }

    /// Build a scratch bare repo with a `memories/` tree several
    /// levels deep, so a listing that only reached the first level
    /// could never accidentally pass: `memories/a/one.md`,
    /// `memories/a/b/two.md`, `memories/a/b/c/three.md`,
    /// `memories/x/four.md`. Five directory nodes are reachable under
    /// `memories/`: itself, `a`, `a/b`, `a/b/c`, and `x`.
    fn fixture_repo_with_nested_memories() -> TempDir {
        let tmp = TempDir::new().expect("tempdir");
        let repo_path = tmp.path().join("scratch.git");
        gix::init_bare(&repo_path).expect("init bare scratch repo");
        let repo = open_thread_local(&repo_path);
        write_commit(
            &repo,
            CommitSpec::mmcp_commit(
                "seed nested memories tree",
                vec![
                    ("memories/a/one.md".to_string(), Some(b"one".to_vec())),
                    ("memories/a/b/two.md".to_string(), Some(b"two".to_vec())),
                    (
                        "memories/a/b/c/three.md".to_string(),
                        Some(b"three".to_vec()),
                    ),
                    ("memories/x/four.md".to_string(), Some(b"four".to_vec())),
                ],
                "Test Author",
                "test@example.com",
            ),
        )
        .expect("seed commit");
        tmp
    }

    /// Falsification: [`list_tree_recursive`] resolves the commit
    /// exactly once for its whole descent, regardless of how many
    /// directory nodes it reaches, unlike a manual DFS calling
    /// `list_tree` then `list_subtrees` per directory (the shape
    /// `list_memory_slug_dirs` used before migrating to this
    /// primitive), which pays `resolve_rev` twice per node. Both
    /// shapes run in this one test function, sharing
    /// [`RESOLVE_REV_CALLS`] sequentially, so neither call-count
    /// assertion races a concurrently-running test thread touching
    /// the same process-wide counter. Red check performed manually:
    /// replacing the single-traversal body with the per-node loop
    /// below makes the first assertion fail (`10` instead of `1`),
    /// and the second assertion proves the counter is genuinely
    /// capable of reading a value above `1` on this fixture, not
    /// just always `1` regardless of what ran.
    #[test]
    fn list_tree_recursive_resolves_the_commit_exactly_once() {
        let tmp = fixture_repo_with_nested_memories();
        let repo_path = tmp.path().join("scratch.git");
        let rev = Rev::head();

        RESOLVE_REV_CALLS.store(0, Ordering::SeqCst);
        let repo = open_thread_local(&repo_path);
        let mut dirs = list_tree_recursive(&repo, "memories", &rev).expect("list_tree_recursive");
        dirs.sort_by(|a, b| a.0.cmp(&b.0));

        assert_eq!(
            RESOLVE_REV_CALLS.load(Ordering::SeqCst),
            1,
            "a single recursive descent must resolve the commit exactly once"
        );

        let expected: Vec<TreeDirEntry> = vec![
            (String::new(), vec![]),
            ("a".to_string(), vec!["one.md".to_string()]),
            ("a/b".to_string(), vec!["two.md".to_string()]),
            ("a/b/c".to_string(), vec!["three.md".to_string()]),
            ("x".to_string(), vec!["four.md".to_string()]),
        ];
        assert_eq!(
            dirs, expected,
            "every directory reached must carry its own direct blob names"
        );

        // Same fixture, the OLD per-node shape: one `list_tree` plus
        // one `list_subtrees` call per directory, each independently
        // resolving the commit.
        RESOLVE_REV_CALLS.store(0, Ordering::SeqCst);
        let repo = open_thread_local(&repo_path);
        let mut visited = 0usize;
        let mut stack = vec!["memories".to_string()];
        while let Some(prefix) = stack.pop() {
            visited += 1;
            let _files = list_tree(&repo, &prefix, &rev).expect("list_tree");
            let subs = list_subtrees(&repo, &prefix, &rev).expect("list_subtrees");
            for name in subs {
                stack.push(format!("{prefix}/{name}"));
            }
        }

        assert_eq!(visited, 5, "fixture must have exactly 5 directory nodes");
        assert_eq!(
            RESOLVE_REV_CALLS.load(Ordering::SeqCst),
            visited * 2,
            "the per-node shape must pay 2 resolve_rev calls per directory node"
        );
    }
}
