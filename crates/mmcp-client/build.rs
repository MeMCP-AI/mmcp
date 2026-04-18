fn main() {
    // Force this build script to run before every compile. The
    // default behaviour ("re-run only when build.rs changes")
    // skipped the exe-stash pass on source edits of the
    // binary, which let the linker fire against a still-locked
    // `mmcp.exe` and fail with ERROR_ACCESS_DENIED. Depending
    // on a nonexistent sentinel forces a re-run every build.
    println!("cargo:rerun-if-changed=.mmcp-stash-sentinel-never-exists");
    if !cfg!(windows) { return; }
    #[cfg(windows)] win::stash();
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
        let base = env::var("CARGO_BIN_NAME_OVERRIDE").ok()
            .or_else(|| {
                let manifest = env::var("CARGO_MANIFEST_DIR").ok()?;
                let m = cargo_toml::Manifest::from_path(
                    PathBuf::from(manifest).join("Cargo.toml")
                ).ok()?;
                // First explicitly-named bin wins; fall back to package name.
                m.bin.into_iter().find_map(|b| b.name)
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

        let Some(profile_dir) = target_profile_dir() else { return };
        let target = profile_dir.join(exe_name());
        let prefix = stash_prefix();

        // Sweep prior stashes. Every stash whose old process has
        // finished is no longer held; free it up now so the temp
        // directory doesn't accumulate one leftover per rebuild.
        sweep_unheld_stashes(&prefix);

        if !target.exists() { return; }

        let stash = match tempfile::Builder::new()
            .prefix(&prefix)
            .suffix(env::consts::EXE_SUFFIX)
            .rand_bytes(16)
            .disable_cleanup(true)
            .make(|p| fs::File::create(p))
        {
            Ok(f) => f.path().to_path_buf(),
            Err(e) => { println!("cargo:warning=temp create failed: {e}"); return; }
        };
        let _ = fs::remove_file(&stash);

        if let Err(e) = fs::rename(&target, &stash) {
            println!("cargo:warning=cannot stash locked exe: {e}");
            return;
        }
    }

    /// Walk `env::temp_dir()` and delete every stash that starts
    /// with `prefix` and is not currently held open.
    ///
    /// Windows returns:
    /// - `ERROR_SHARING_VIOLATION` (os error 32) when another
    ///   process has the .exe open for execution — the expected
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