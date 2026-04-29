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
    match &cfg.sync {
        Some(sync) => println!("server        : {}", sync.server_url),
        None => println!("server        : (local-only)"),
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
