//! `mmcp status` implementation.

use anyhow::{Context, Result};

use mmcp_store::config::{find_project_root, load};

/// Print a compact human-readable summary of the project state.
pub async fn run() -> Result<()> {
    let cwd = std::env::current_dir().context("reading current working directory")?;
    let root = find_project_root(&cwd)
        .context("no mmcp project found in current directory or any parent")?;
    let cfg = load(&root)?;

    println!("project root  : {}", root.display());
    println!("project uuid  : {}", cfg.project_uuid);
    // Full effective-remote-set display (merging user config, naming
    // each remote, flagging the default) is wave 3's job per FR-301;
    // this mechanical adaptation only keeps `mmcp status` compiling
    // against the now-always-defaulted `SyncConfig` shape.
    match cfg.sync.server_url.as_deref() {
        Some(url) => println!("server        : {url}"),
        None if cfg.sync.remotes.is_empty() => println!("server        : (local-only)"),
        None => println!(
            "server        : {} remote(s) configured",
            cfg.sync.remotes.len()
        ),
    }
    let subs = &cfg.subscriptions;
    println!(
        "default group : {}",
        if subs.no_default_global {
            "disabled"
        } else {
            "global"
        }
    );
    if !subs.groups.is_empty() {
        println!("subs groups   : {}", subs.groups.join(", "));
    }
    if !subs.languages.is_empty() || subs.auto_detect_languages {
        println!(
            "subs langs    : use={}, auto_detect={}",
            if subs.languages.is_empty() {
                "(none)".to_string()
            } else {
                subs.languages.join(",")
            },
            subs.auto_detect_languages
        );
    }
    if !subs.memories.is_empty() {
        println!("subs memories : {}", subs.memories.join(", "));
    }
    if !subs.tags.is_empty() {
        println!("subs tags     : {}", subs.tags.join(", "));
    }
    Ok(())
}
