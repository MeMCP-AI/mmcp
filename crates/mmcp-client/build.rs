fn main() {
    emit_build_info();

    // Force this build script to run before every compile. The
    // default behaviour ("re-run only when build.rs changes")
    // skipped the exe-stash pass on source edits of the
    // binary, which let the linker fire against a still-locked
    // `mmcp.exe` and fail with ERROR_ACCESS_DENIED. Depending
    // on a nonexistent sentinel forces a re-run every build.
    println!("cargo:rerun-if-changed=.mmcp-stash-sentinel-never-exists");
    if !cfg!(windows) {
        return;
    }
    #[cfg(windows)]
    win::stash();
}

/// `version` MCP tool support: embed the rustc toolchain semver and
/// the git SHA / describe string as compile-time `cargo:rustc-env`
/// vars, read back via `option_env!` in `commands/serve.rs`.
///
/// Runs once, here, at build time only -- `vergen-gitcl` shells out
/// to the local `git` CLI during THIS build script, never from the
/// running binary at request time, so the shipped tool has zero
/// runtime cost and zero runtime git dependency. Picked over
/// `vergen-git2` (pulls in libgit2, a C-binding `*-sys` crate) and
/// `vergen-gix` (resolves a second, unpatched copy of gitoxide
/// alongside the workspace's patched `gix` fork pin in the root
/// `Cargo.toml`, doubling an already-large dependency tree).
///
/// When git info is unavailable (no `.git`, no `git` on PATH,
/// building from a source tarball), `vergen-gitcl` does not fail
/// the build: it emits the literal sentinel `VERGEN_IDEMPOTENT_OUTPUT`
/// for the affected vars instead. `commands/serve.rs`'s `version`
/// tool recognises that sentinel and reports the field as absent
/// rather than presenting it as a real commit SHA.
fn emit_build_info() {
    use vergen_gitcl::{Emitter, GitclBuilder, RustcBuilder};

    let result: anyhow::Result<()> = (|| {
        let gitcl = GitclBuilder::all_git()?;
        let rustc = RustcBuilder::all_rustc()?;
        Emitter::default()
            .add_instructions(&gitcl)?
            .add_instructions(&rustc)?
            .emit()
    })();
    if let Err(e) = result {
        // Non-fatal: the `version` tool falls back to reporting
        // only `CARGO_PKG_VERSION` when these compile-time env vars
        // never got set.
        println!("cargo:warning=vergen-gitcl instruction emission failed: {e}");
    }
}

#[cfg(windows)]
mod win {
    use std::{env, fs, path::PathBuf};

    fn target_profile_dir() -> Option<PathBuf> {
        // OUT_DIR = <target>/<triple?>/<profile>/build/<crate>-<hash>/out
        // Pop 3 components → <target>/<triple?>/<profile>
        let out_dir = PathBuf::from(env::var_os("OUT_DIR")?);
        let profile_dir = out_dir.parent()?.parent()?.parent()?.to_path_buf();
        Some(profile_dir)
    }

    fn exe_name() -> String {
        let base = env::var("CARGO_BIN_NAME_OVERRIDE")
            .ok()
            .or_else(|| {
                let manifest = env::var("CARGO_MANIFEST_DIR").ok()?;
                let m = cargo_toml::Manifest::from_path(PathBuf::from(manifest).join("Cargo.toml"))
                    .ok()?;
                // First explicitly-named bin wins; fall back to package name.
                m.bin
                    .into_iter()
                    .find_map(|b| b.name)
                    .or_else(|| m.package.map(|p| p.name))
            })
            .or_else(|| env::var("CARGO_PKG_NAME").ok())
            .unwrap_or_else(|| "unknown".into());
        format!("{base}{}", env::consts::EXE_SUFFIX)
    }

    fn stash_prefix() -> String {
        let pkg = env::var("CARGO_PKG_NAME").unwrap_or_else(|_| "bin".into());
        format!("{pkg}-stash-")
    }

    pub fn stash() {
        // Re-run if the override env var changes.
        println!("cargo:rerun-if-env-changed=CARGO_BIN_NAME_OVERRIDE");

        let Some(profile_dir) = target_profile_dir() else {
            return;
        };
        let target = profile_dir.join(exe_name());
        let prefix = stash_prefix();

        // Sweep prior stashes. Every stash whose old process has
        // finished is no longer held; free it up now so the temp
        // directory doesn't accumulate one leftover per rebuild.
        sweep_unheld_stashes(&prefix);

        if !target.exists() {
            return;
        }

        let stash = match tempfile::Builder::new()
            .prefix(&prefix)
            .suffix(env::consts::EXE_SUFFIX)
            .rand_bytes(16)
            .disable_cleanup(true)
            .make(|p| fs::File::create(p))
        {
            Ok(f) => f.path().to_path_buf(),
            Err(e) => {
                println!("cargo:warning=temp create failed: {e}");
                return;
            }
        };
        let _ = fs::remove_file(&stash);

        relocate(&target, &stash);
    }

    /// Windows `ERROR_NOT_SAME_DEVICE`: `MoveFileW` (what `fs::rename`
    /// calls into) refuses to relocate a file across two different
    /// volumes. This fires whenever `CARGO_TARGET_DIR` resolves to a
    /// drive other than the one holding `env::temp_dir()` (e.g. project
    /// checkout on `P:`, target dir on `G:`): the rename is structurally
    /// impossible, not blocked by the file lock the stash exists to work
    /// around, so it always needs the copy-then-delete fallback below.
    const ERROR_NOT_SAME_DEVICE: i32 = 17;

    /// Move `target` to `stash`, falling back to copy-then-delete when
    /// the two paths sit on different drives (see
    /// `ERROR_NOT_SAME_DEVICE`). A copy still succeeds against a
    /// currently-executing `.exe` because execution only requires
    /// `FILE_SHARE_READ`/`FILE_SHARE_DELETE`, the same sharing mode a
    /// same-drive rename already relies on; deleting the original
    /// afterward relies on the same NTFS POSIX-delete semantics a
    /// same-drive rename depends on, so a still-running image can, in
    /// principle, reject the delete, which surfaces as its own
    /// "cleanup failed" warning rather than silently leaving a stale
    /// copy. Any other rename failure keeps surfacing as the
    /// pre-existing warning, unchanged.
    fn relocate(target: &std::path::Path, stash: &std::path::Path) {
        if let Err(e) = fs::rename(target, stash) {
            if e.raw_os_error() != Some(ERROR_NOT_SAME_DEVICE) {
                println!("cargo:warning=cannot stash locked exe: {e}");
                return;
            }
            if let Err(copy_err) = fs::copy(target, stash) {
                println!(
                    "cargo:warning=cannot stash locked exe across drives (copy failed): {copy_err}"
                );
                return;
            }
            if let Err(rm_err) = fs::remove_file(target) {
                println!(
                    "cargo:warning=cannot stash locked exe across drives (cleanup failed): {rm_err}"
                );
            }
        }
    }

    /// Walk `env::temp_dir()` and delete every stash that starts
    /// with `prefix` and is not currently held open.
    ///
    /// Windows returns:
    /// - `ERROR_SHARING_VIOLATION` (os error 32) when another
    ///   process has the .exe open for execution: the expected
    ///   "still held, retry next build" case.
    /// - `ERROR_ACCESS_DENIED` (os error 5) occasionally surfaces
    ///   for the same scenario under stricter ACL setups.
    ///
    /// Both are silent. Any other error (unexpected filesystem
    /// state, permission problem unrelated to execution lock) is
    /// surfaced as `cargo:warning=` so a real problem doesn't hide
    /// behind a silent `let _ =`.
    fn sweep_unheld_stashes(prefix: &str) {
        let Ok(entries) = fs::read_dir(env::temp_dir()) else {
            return;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            if !name.to_string_lossy().starts_with(prefix) {
                continue;
            }
            let path = entry.path();
            match fs::remove_file(&path) {
                Ok(()) => {}
                Err(err) if matches!(err.raw_os_error(), Some(32) | Some(5)) => {
                    // Still held by a running old binary. Leave
                    // for the next build to retry.
                }
                Err(err) => {
                    println!(
                        "cargo:warning=failed to delete stale stash {}: {err}",
                        path.display()
                    );
                }
            }
        }
    }
}
