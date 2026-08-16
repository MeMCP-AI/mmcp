//! Synchronous repository operations against a bare git repo.
//!
//! These functions are invoked from `spawn_blocking` inside the async
//! backend so the whole `gix` call tree stays synchronous. Every
//! function that needs an open repository takes an already-opened
//! `&gix::Repository`: [`crate::native::NativeBackend`] owns the
//! cached [`gix::ThreadSafeRepository`] handle and hands out a
//! thread-local view per call, so this module never re-opens a repo
//! itself.

use std::ffi::OsString;
use std::path::Path;
use std::process::Command;

use bytes::Bytes;
use gix::bstr::BString;
use gix::objs::tree::EntryKind;

use crate::error::GitError;
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
/// never argv. Does nothing for [`Credentials::None`] so the ambient
/// git env (SSH agent, credential helper, `.netrc`) remains in charge,
/// exactly matching [`Credentials::None`]'s own doc comment: "rely on
/// the ambient git environment ... the right choice for interactive
/// use". No interactive-prompt suppression is applied on this arm, on
/// purpose — suppressing it here would silently break that documented
/// contract for a legitimate interactive CLI caller.
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
/// already accepts for its own value. Also calls
/// [`suppress_interactive_prompts`]: mmcp already supplied the
/// definitive credential on this arm, so any interactive fallback
/// (a GCM popup, an SSH passphrase prompt) could only mask a genuine
/// auth failure behind a hung subprocess, never serve a legitimate
/// purpose, in an automated caller that has no human present to
/// answer it.
///
/// For `SshCommand`, sets `GIT_SSH_COMMAND` and, for the same reason
/// as `BearerHttp`, also calls [`suppress_interactive_prompts`].
///
/// Any `-c` flags must appear *before* the git subcommand, so this
/// helper is called on a freshly-constructed `Command` before its
/// `.arg("clone")` / `.arg("fetch")` / etc.
fn apply_credentials(cmd: &mut Command, creds: &Credentials) {
    match creds {
        Credentials::None => {}
        Credentials::BearerHttp(token) => {
            cmd.env("GIT_CONFIG_COUNT", "1");
            cmd.env("GIT_CONFIG_KEY_0", "http.extraHeader");
            cmd.env(
                "GIT_CONFIG_VALUE_0",
                format!("Authorization: Bearer {token}"),
            );
            suppress_interactive_prompts(cmd);
        }
        Credentials::SshCommand(value) => {
            cmd.env("GIT_SSH_COMMAND", value);
            suppress_interactive_prompts(cmd);
        }
    }
}

/// Suppress every interactive-credential fallback a `git` subprocess
/// might otherwise reach for: the terminal prompt, `GIT_ASKPASS`, and
/// any configured `credential.helper` (on Windows, typically Git
/// Credential Manager, which pops a real desktop dialog). Called only
/// from [`apply_credentials`]'s `BearerHttp` and `SshCommand` arms,
/// where mmcp already supplied a definitive credential and an
/// interactive fallback could only mask a real auth failure — never
/// from the `Credentials::None` arm, whose whole documented purpose is
/// to defer to that same ambient, interactive machinery.
///
/// `-c credential.helper=` (empty value) disables every configured
/// helper for this invocation only, without touching the user's
/// global git config. `GIT_TERMINAL_PROMPT=0` stops git's own
/// built-in terminal prompt. `GIT_ASKPASS=""` (empty program path)
/// makes git treat askpass as unconfigured rather than trying to
/// execute an empty command.
fn suppress_interactive_prompts(cmd: &mut Command) {
    cmd.arg("-c").arg("credential.helper=");
    cmd.env("GIT_TERMINAL_PROMPT", "0");
    cmd.env("GIT_ASKPASS", "");
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

/// Clone `remote_url` into `dst` via the user-installed git binary.
pub fn clone(remote_url: &str, dst: &Path, creds: &Credentials) -> Result<(), GitError> {
    let mut cmd = Command::new(git_binary());
    apply_credentials(&mut cmd, creds);
    let output = cmd.arg("clone").arg(remote_url).arg(dst).output()?;
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
/// `repo_path`. An `origin` remote is configured on the fly so
/// subsequent fetches reuse it.
pub fn fetch(
    repo_path: &Path,
    remote_url: &str,
    refspecs: &[String],
    creds: &Credentials,
) -> Result<(), GitError> {
    ensure_remote(repo_path, remote_url)?;
    let mut cmd = Command::new(git_binary());
    apply_credentials(&mut cmd, creds);
    cmd.arg("-C").arg(repo_path).arg("fetch").arg("origin");
    for spec in refspecs {
        cmd.arg(spec);
    }
    let output = cmd.output()?;
    if !output.status.success() {
        return Err(GitError::transport(
            "fetch",
            remote_url,
            String::from_utf8_lossy(&output.stderr).into_owned(),
        ));
    }
    Ok(())
}

/// Push `refspecs` (local, remote, force) from `repo` to `remote_url`.
/// `repo` is used only for the local-ref preflight check; the push
/// itself still shells out to the `git` binary so it works against
/// any smart-HTTP-capable remote.
pub fn push(
    repo: &gix::Repository,
    remote_url: &str,
    refspecs: &[(String, String, bool)],
    creds: &Credentials,
) -> Result<PushReport, GitError> {
    preflight_local_refs(repo, refspecs)?;
    let repo_path = repo.git_dir();
    ensure_remote(repo_path, remote_url)?;
    let mut cmd = Command::new(git_binary());
    apply_credentials(&mut cmd, creds);
    cmd.arg("-C").arg(repo_path).arg("push").arg("origin");
    for (local, remote, force) in refspecs {
        let spec = if *force {
            format!("+{local}:{remote}")
        } else {
            format!("{local}:{remote}")
        };
        cmd.arg(spec);
    }
    let output = cmd.output()?;
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
fn preflight_local_refs(
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
fn ensure_remote(repo_path: &Path, remote_url: &str) -> Result<(), GitError> {
    // Try to set the URL first; if the remote does not exist,
    // fall back to adding it.
    let set = Command::new(git_binary())
        .arg("-C")
        .arg(repo_path)
        .arg("remote")
        .arg("set-url")
        .arg("origin")
        .arg(remote_url)
        .output()?;
    if set.status.success() {
        return Ok(());
    }
    let add = Command::new(git_binary())
        .arg("-C")
        .arg(repo_path)
        .arg("remote")
        .arg("add")
        .arg("origin")
        .arg(remote_url)
        .output()?;
    if !add.status.success() {
        return Err(GitError::transport(
            "remote-add",
            remote_url,
            String::from_utf8_lossy(&add.stderr).into_owned(),
        ));
    }
    Ok(())
}

/// Resolve a `Rev` to a concrete commit object id.
fn resolve_rev(repo: &gix::Repository, rev: &Rev) -> Result<gix::ObjectId, GitError> {
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

/// Node in the in-memory tree we build before flushing to git.
///
/// Directories hold a map from child name to another node. Leaves
/// hold a blob object id. Pending deletes are represented as
/// `Option::None` blob nodes that the flusher drops.
enum TreeNode {
    Dir(std::collections::BTreeMap<BString, TreeNode>),
    Blob(Option<gix::ObjectId>),
}

impl TreeNode {
    fn empty_dir() -> Self {
        TreeNode::Dir(std::collections::BTreeMap::new())
    }
}

/// Load `tree_id` into an in-memory [`TreeNode::Dir`] recursively.
fn load_tree(repo: &gix::Repository, tree_id: gix::ObjectId) -> Result<TreeNode, GitError> {
    let obj = repo.find_object(tree_id).map_err(commit_err)?;
    let tree: gix::objs::Tree = obj.into_tree().decode().map_err(commit_err)?.into();
    let mut entries = std::collections::BTreeMap::new();
    for entry in tree.entries {
        let node = match entry.mode.kind() {
            EntryKind::Tree => load_tree(repo, entry.oid)?,
            EntryKind::Blob | EntryKind::BlobExecutable => TreeNode::Blob(Some(entry.oid)),
            _ => continue,
        };
        entries.insert(entry.filename.clone(), node);
    }
    Ok(TreeNode::Dir(entries))
}

/// Walk `node` (must be a dir) following `components`, creating
/// intermediate directories as needed, and apply the given leaf edit.
fn apply_edit(node: &mut TreeNode, components: &[&str], leaf: TreeNode) -> Result<(), GitError> {
    let TreeNode::Dir(map) = node else {
        return Err(commit_msg_err(
            "path component collides with an existing blob",
        ));
    };
    let Some((head, rest)) = components.split_first() else {
        return Err(commit_msg_err("empty path component in commit file list"));
    };
    let key = BString::from(*head);
    if rest.is_empty() {
        if matches!(leaf, TreeNode::Blob(None)) {
            map.remove(&key);
        } else {
            map.insert(key, leaf);
        }
        return Ok(());
    }
    let child = map.entry(key).or_insert_with(TreeNode::empty_dir);
    apply_edit(child, rest, leaf)
}

/// Recursively flush an in-memory tree to the object database.
fn flush_tree(repo: &gix::Repository, node: &TreeNode) -> Result<Option<gix::ObjectId>, GitError> {
    let TreeNode::Dir(map) = node else {
        return Err(commit_msg_err("flush_tree expects a Dir"));
    };
    let mut entries: Vec<gix::objs::tree::Entry> = Vec::new();
    for (name, child) in map {
        match child {
            TreeNode::Blob(Some(oid)) => {
                entries.push(gix::objs::tree::Entry {
                    mode: EntryKind::Blob.into(),
                    filename: name.clone(),
                    oid: *oid,
                });
            }
            TreeNode::Blob(None) => {}
            TreeNode::Dir(_) => {
                if let Some(sub_id) = flush_tree(repo, child)? {
                    entries.push(gix::objs::tree::Entry {
                        mode: EntryKind::Tree.into(),
                        filename: name.clone(),
                        oid: sub_id,
                    });
                }
            }
        }
    }
    if entries.is_empty() {
        return Ok(None);
    }
    entries.sort();
    let tree = gix::objs::Tree { entries };
    Ok(Some(repo.write_object(&tree).map_err(commit_err)?.detach()))
}

/// Construct a new tree by applying the edits in `files` on top of
/// the parent tree. Returns the new tree object id.
fn build_tree(
    repo: &gix::Repository,
    parent_tree_id: Option<gix::ObjectId>,
    files: &[(String, Option<Vec<u8>>)],
) -> Result<gix::ObjectId, GitError> {
    let mut root = match parent_tree_id {
        Some(parent) => load_tree(repo, parent)?,
        None => TreeNode::empty_dir(),
    };

    for (path, contents) in files {
        let components: Vec<&str> = path.split('/').filter(|c| !c.is_empty()).collect();
        if components.is_empty() {
            return Err(GitError::PathNotFound(path.clone()));
        }
        let leaf = match contents {
            Some(bytes) => {
                let blob_id = repo
                    .write_blob(bytes.as_slice())
                    .map_err(commit_err)?
                    .detach();
                TreeNode::Blob(Some(blob_id))
            }
            None => TreeNode::Blob(None),
        };
        apply_edit(&mut root, &components, leaf)?;
    }

    match flush_tree(repo, &root)? {
        Some(id) => Ok(id),
        None => {
            // Empty tree: write a zero-entry tree object.
            let empty = gix::objs::Tree {
                entries: Vec::new(),
            };
            Ok(repo.write_object(&empty).map_err(commit_err)?.detach())
        }
    }
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
    use super::*;

    /// Collect every `(key, value)` env var `apply_credentials` set on
    /// `cmd`, so a test can assert presence/absence and value without
    /// depending on `std::process::Command`'s (partial) `Debug` output.
    fn envs_of(cmd: &Command) -> std::collections::HashMap<String, String> {
        cmd.get_envs()
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
        cmd.get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    /// Falsification: `Credentials::None` must leave the subprocess
    /// exactly as the ambient environment would — no suppression args,
    /// no suppression env vars — matching its own doc comment's
    /// promise that this is "the right choice for interactive use".
    #[test]
    fn credentials_none_applies_no_suppression_and_no_credential_env() {
        let mut cmd = Command::new("git");
        apply_credentials(&mut cmd, &Credentials::None);

        assert!(
            args_of(&cmd).is_empty(),
            "Credentials::None must not add any -c flags"
        );
        let envs = envs_of(&cmd);
        assert!(
            !envs.contains_key("GIT_TERMINAL_PROMPT"),
            "Credentials::None must leave GIT_TERMINAL_PROMPT untouched"
        );
        assert!(
            !envs.contains_key("GIT_ASKPASS"),
            "Credentials::None must leave GIT_ASKPASS untouched"
        );
        assert!(
            !envs.contains_key("GIT_CONFIG_COUNT"),
            "Credentials::None must not set any bearer config env"
        );
    }

    /// Falsification: `Credentials::BearerHttp` must both apply the
    /// bearer header AND suppress every interactive fallback, so an
    /// automated caller with a definitive (if possibly stale/invalid)
    /// token never blocks on a GCM/terminal/askpass prompt.
    #[test]
    fn credentials_bearer_http_suppresses_interactive_prompts() {
        let mut cmd = Command::new("git");
        apply_credentials(&mut cmd, &Credentials::BearerHttp("s3cr3t".to_string()));

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
    }

    /// Falsification: `Credentials::SshCommand` must also suppress
    /// interactive fallbacks, for the same automated-caller reasoning
    /// as `BearerHttp`.
    #[test]
    fn credentials_ssh_command_suppresses_interactive_prompts() {
        let mut cmd = Command::new("git");
        apply_credentials(
            &mut cmd,
            &Credentials::SshCommand("ssh -i /key".to_string()),
        );

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
}
