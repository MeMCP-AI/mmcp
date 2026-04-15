//! `mmcp sync`, `mmcp pull`, and `mmcp push` implementations.

use anyhow::{Context, Result, bail};

use crate::config::{find_project_root, load};

/// Run the sync engine.
///
/// `pull` and `push` may be toggled independently so that `mmcp
/// pull` and `mmcp push` reuse the same code path.
pub async fn run(pull: bool, push: bool) -> Result<()> {
    let cwd = std::env::current_dir().context("reading current working directory")?;
    let root = find_project_root(&cwd)
        .context("no mmcp project found in current directory or any parent")?;
    let cfg = load(&root)?;

    let Some(sync) = cfg.sync.as_ref() else {
        bail!(
            "project {} has no [sync] block; cannot sync against a remote",
            cfg.project_uuid
        );
    };

    if pull {
        tracing::info!(server = %sync.server_url, "pull: not yet wired to the server");
    }
    if push {
        tracing::info!(server = %sync.server_url, "push: not yet wired to the server");
    }
    println!(
        "sync against {} (pull={}, push={}) is a stub in this build",
        sync.server_url, pull, push
    );
    Ok(())
}
