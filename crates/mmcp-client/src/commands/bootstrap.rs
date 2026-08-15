//! CLI surface for `mmcp bootstrap`, the human-readable
//! counterpart to the `mcp:bootstrap_context` MCP tool. Mirrors its
//! instruction-only shape: prints the groups the
//! current project is allowed to enumerate (`groups_in_scope`),
//! the addresses pinned by `[subscriptions]`
//! (`subscribed_reads`), and the four-axis subscription summary.
//!
//! Memory bodies and metadata are deliberately not rendered.
//! Operators reach for `mmcp memory list <group>` to enumerate a
//! group, then `mmcp memory read <group> <slug>` to fetch a body.

use anyhow::Result;
use clap::Args;
use mmcp_core::config::is_group_adopted;
use mmcp_store::config::{find_project_root, load as load_project_config};
use mmcp_store::home::MmcpHome;
use mmcp_store::memory::resolve_group;
use uuid::Uuid;

use crate::commands::serve::resolve_subscribed_reads;
use crate::notes::render_notes_tail;

#[derive(Debug, Args)]
pub struct BootstrapArgs {
    /// Explicit project group selector (UUID or slug). When set,
    /// resolves against the local mirror without touching the
    /// filesystem; the printed report omits `subscribed_reads` and
    /// the subscriptions summary.
    #[arg(long)]
    pub project: Option<String>,

    /// Project root override. When set, walks this path for
    /// `.mmcp.toml` instead of cwd. Mirrors the MCP tool's `path`
    /// arg.
    #[arg(long)]
    pub path: Option<std::path::PathBuf>,
}

pub async fn run(args: BootstrapArgs) -> Result<()> {
    let home = MmcpHome::discover()?;
    let (backend, groups) = home.init_backend().await?;

    // Resolve the project group + config. Explicit selector wins;
    // `path` walks the supplied directory; otherwise walk cwd.
    let (project_uuid, project_cfg, project_root) = match args.project.as_deref() {
        Some(query) => {
            let entry = resolve_group(&groups, query)
                .await
                .map_err(anyhow::Error::from)?;
            (Some(*entry.manifest.group_id.as_uuid()), None, None)
        }
        None => {
            let starting = match args.path.as_deref() {
                Some(p) => Some(p.to_path_buf()),
                None => std::env::current_dir().ok(),
            };
            let project_root = starting.and_then(|dir| find_project_root(&dir));
            let project_cfg = project_root
                .as_ref()
                .and_then(|root| load_project_config(root).ok());
            let project_uuid = project_cfg.as_ref().map(|cfg| *cfg.project_uuid.as_uuid());
            (project_uuid, project_cfg, project_root)
        }
    };

    let entries = groups.list().await;

    // Which Shared groups has this project subscribed to?
    let adopted_shared: std::collections::HashSet<Uuid> = match project_cfg.as_ref() {
        None => std::collections::HashSet::new(),
        Some(cfg) => entries
            .iter()
            .filter(|entry| entry.manifest.scope == mmcp_core::manifest::GroupScope::Shared)
            .filter(|entry| is_group_adopted(&entry.manifest.slug, cfg))
            .map(|entry| *entry.manifest.group_id.as_uuid())
            .collect(),
    };

    let mut groups_in_scope: Vec<(Uuid, String, &'static str)> = Vec::new();
    for entry in &entries {
        let entry_uuid = *entry.manifest.group_id.as_uuid();
        let scope_label: &'static str = match entry.manifest.scope {
            mmcp_core::manifest::GroupScope::Global => "global",
            mmcp_core::manifest::GroupScope::Shared => "shared",
            mmcp_core::manifest::GroupScope::Project => "project",
        };
        let in_scope = match entry.manifest.scope {
            mmcp_core::manifest::GroupScope::Global => true,
            mmcp_core::manifest::GroupScope::Shared => adopted_shared.contains(&entry_uuid),
            mmcp_core::manifest::GroupScope::Project => project_uuid == Some(entry_uuid),
        };
        if !in_scope {
            continue;
        }
        groups_in_scope.push((entry_uuid, entry.manifest.slug.clone(), scope_label));
    }

    if let Some(root) = project_root.as_ref() {
        println!("project root  : {}", root.display());
    }
    if let Some(uuid) = project_uuid {
        println!("project uuid  : {uuid}");
    }
    println!();

    println!("Groups in scope:");
    if groups_in_scope.is_empty() {
        println!("  (none)");
    } else {
        for (uuid, slug, scope) in &groups_in_scope {
            println!("  {slug}  [{scope}]  {uuid}");
        }
    }

    let (subscribed_reads, subscribed_notes) = match project_cfg.as_ref() {
        None => (Vec::new(), Vec::new()),
        Some(cfg) => {
            resolve_subscribed_reads(&backend, &entries, cfg, &adopted_shared, project_uuid).await
        }
    };
    println!();
    println!("Subscribed reads:");
    if subscribed_reads.is_empty() {
        println!("  (none)");
    } else {
        for entry in &subscribed_reads {
            let group = entry.get("group").and_then(|v| v.as_str()).unwrap_or("?");
            match entry.get("kind").and_then(|v| v.as_str()) {
                Some("group") => {
                    let slug = entry.get("slug").and_then(|v| v.as_str()).unwrap_or("?");
                    let count = entry.get("count").and_then(|v| v.as_u64()).unwrap_or(0);
                    let hint = entry
                        .get("fetch_hint")
                        .and_then(|v| v.as_str())
                        .unwrap_or("?");
                    println!("  [group]  {slug}  {group}  ({count} memories, {hint})");
                }
                Some("memory") => {
                    let slug = entry.get("slug").and_then(|v| v.as_str()).unwrap_or("?");
                    println!("  [memory] {group}/{slug}");
                }
                other => {
                    // An unrecognized shape prints a visible marker instead of silently degrading to `?`.
                    // This flags a future third entry shape instead of letting it vanish unnoticed from this printer.
                    println!(
                        "  [unrecognized kind {other:?}] {entry}, printer needs updating for this shape"
                    );
                }
            }
        }
    }
    render_notes_tail(&subscribed_notes);

    if let Some(cfg) = project_cfg.as_ref() {
        println!();
        println!("Subscriptions:");
        let s = &cfg.subscriptions;
        if !s.tags.is_empty() {
            println!("  tags      : {}", s.tags.join(", "));
        }
        if !s.memories.is_empty() {
            println!("  memories  : {}", s.memories.join(", "));
        }
        if !s.groups.is_empty() {
            println!("  groups    : {}", s.groups.join(", "));
        }
        if !s.languages.is_empty() {
            println!("  languages : {}", s.languages.join(", "));
        }
        if s.tags.is_empty()
            && s.memories.is_empty()
            && s.groups.is_empty()
            && s.languages.is_empty()
        {
            println!("  (none — use `mmcp subscribe <kind> <value>` to add)");
        }
    }

    println!();
    println!(
        "Next: `mmcp memory list <group>` to enumerate, `mmcp subscribe <kind> <value>` to pin."
    );
    Ok(())
}
