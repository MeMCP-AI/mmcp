//! `mmcp status` implementation.

use anyhow::{Context, Result};

use mmcp_core::config::ProjectConfig;
use mmcp_store::config::{find_project_root, load};
use mmcp_store::home::MmcpHome;
use mmcp_store::{EffectiveRemotes, RemoteLevel, resolve_effective_remotes};

/// Print a compact human-readable summary of the project state.
pub async fn run() -> Result<()> {
    let cwd = std::env::current_dir().context("reading current working directory")?;
    let root = find_project_root(&cwd)
        .context("no mmcp project found in current directory or any parent")?;
    let cfg = load(&root)?;

    println!("project root  : {}", root.display());
    println!("project uuid  : {}", cfg.project_uuid);
    println!("remote-only   : {}", cfg.project_remote_only);
    print_remotes_section(&cfg);

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

/// Print the effective remote-set section: every remote in the
/// merged user+project set (`mmcp_store::resolve_effective_remotes`,
/// per FR-301's precedence rules), naming each one's kind and origin
/// level and marking the resolved default.
///
/// A resolution failure (name collision, ambiguous default, a
/// user-level `direct-git` remote missing its required `group`) is
/// printed inline as this section's own content instead of aborting
/// the rest of the report: `mmcp status` is the diagnostic tool for
/// exactly these misconfigurations, so every other section above and
/// below still prints normally.
fn print_remotes_section(project_cfg: &ProjectConfig) {
    let resolved = MmcpHome::discover()
        .and_then(|home| home.load_user_config())
        .and_then(|user_cfg| resolve_effective_remotes(&user_cfg, project_cfg));
    match resolved {
        Ok(effective) => print_effective_remotes(&effective),
        Err(err) => println!("remotes       : CONFIGURATION ERROR - {err}"),
    }
}

/// Render a successfully-resolved [`EffectiveRemotes`] set: a count
/// line, then one indented line per remote naming its name, kind,
/// origin level, and whether it is the resolved default.
fn print_effective_remotes(effective: &EffectiveRemotes) {
    if effective.remotes.is_empty() {
        println!("remotes       : (none configured)");
        return;
    }
    println!("remotes       : {} configured", effective.remotes.len());
    for (index, remote) in effective.remotes.iter().enumerate() {
        let level = match remote.level {
            RemoteLevel::User => "user",
            RemoteLevel::Project => "project",
        };
        let marker = if effective.default_index == Some(index) {
            " (default)"
        } else {
            ""
        };
        println!(
            "  - {name} [{kind}] level={level}{marker}",
            name = remote.name(),
            kind = remote.remote.kind(),
        );
    }
}
