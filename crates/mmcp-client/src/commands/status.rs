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
    println!(
        "default group : {}",
        if cfg.groups.no_default {
            "disabled"
        } else {
            "global"
        }
    );
    if !cfg.groups.additional.is_empty() {
        println!("extra groups  : {}", cfg.groups.additional.join(", "));
    }
    if !cfg.languages.use_.is_empty() || cfg.languages.auto_detect {
        println!(
            "languages     : use={}, auto_detect={}",
            if cfg.languages.use_.is_empty() {
                "(none)".to_string()
            } else {
                cfg.languages.use_.join(",")
            },
            cfg.languages.auto_detect
        );
    }
    Ok(())
}
