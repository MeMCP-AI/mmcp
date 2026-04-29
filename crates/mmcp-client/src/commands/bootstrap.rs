//! CLI surface for `mmcp bootstrap` — the human-readable
//! counterpart to the `mcp:bootstrap_context` MCP tool. Walks every
//! mirrored group, parses each memory's frontmatter, classifies
//! entries as mandatory / project-scoped, and prints a metadata
//! block.
//!
//! Bodies are deliberately not rendered. Operators reach for
//! `mmcp memory read <group> <slug>` after spotting an interesting
//! entry; the bootstrap output is the index, not the content.
//!
//! Scope semantics mirror the MCP tool:
//! - `mandatory` — `frontmatter.mandatory == true` on every group
//!   that's in scope for the current project (FR-025 adoption rule
//!   on Shared-scoped groups is not yet replicated here; the MCP
//!   tool remains canonical for that nuance).
//! - `project` — every memory in the project group.
//! - `all` (default) — union of mandatory + project.

use anyhow::{Context, Result};
use clap::Args;
use mmcp_core::config::ProjectConfig;
use mmcp_core::manifest::GroupScope;
use mmcp_core::memory::MemoryFile;
use mmcp_git::{GitBackend, Rev};
use mmcp_store::config::{find_project_root, load as load_project_config};
use mmcp_store::home::MmcpHome;
use mmcp_store::memory::{list_all_memory_files, resolve_group};

#[derive(Debug, Args)]
pub struct BootstrapArgs {
    /// Restrict the listing: `mandatory`, `project`, or `all`
    /// (default).
    #[arg(long, default_value = "all")]
    pub scope: String,

    /// Explicit project group selector (UUID or slug). Without
    /// this flag the command walks `cwd` for `.mmcp.toml`.
    #[arg(long)]
    pub project: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Scope {
    Mandatory,
    Project,
    All,
}

fn parse_scope(raw: &str) -> Result<Scope> {
    match raw {
        "mandatory" => Ok(Scope::Mandatory),
        "project" => Ok(Scope::Project),
        "all" => Ok(Scope::All),
        other => anyhow::bail!("unknown --scope `{other}` (expected mandatory / project / all)"),
    }
}

pub async fn run(args: BootstrapArgs) -> Result<()> {
    let scope = parse_scope(&args.scope)?;
    let want_mandatory = matches!(scope, Scope::Mandatory | Scope::All);
    let want_project = matches!(scope, Scope::Project | Scope::All);

    let home = MmcpHome::discover()?;
    let (backend, groups) = home.init_backend().await?;

    // Resolve the project group + config. Explicit selector wins;
    // otherwise walk cwd for .mmcp.toml. Mirrors FR-44. The
    // selector branch leaves `project_cfg` as None because an
    // explicit selector does not guarantee the project has a
    // local filesystem root (FR-025 adoption lookup therefore
    // only fires on the cwd-walk branch).
    let (project_uuid, project_cfg) = match args.project.as_deref() {
        Some(query) => {
            let entry = resolve_group(&groups, query)
                .await
                .map_err(anyhow::Error::from)?;
            (Some(*entry.manifest.group_id.as_uuid()), None)
        }
        None => {
            let project_cfg = std::env::current_dir()
                .ok()
                .and_then(|cwd| find_project_root(&cwd))
                .and_then(|root| load_project_config(&root).ok());
            let project_uuid = project_cfg.as_ref().map(|cfg| *cfg.project_uuid.as_uuid());
            (project_uuid, project_cfg)
        }
    };

    let mut mandatory_rows: Vec<Row> = Vec::new();
    let mut project_rows: Vec<Row> = Vec::new();

    for entry in groups.list().await {
        let entry_uuid = *entry.manifest.group_id.as_uuid();
        let is_project = project_uuid == Some(entry_uuid);
        // FR-025: a `mandatory` flag only applies when the
        // owning group is in scope for the current session.
        // Global → always; Shared → only when the project's
        // `.mmcp.toml` lists this group via `groups.additional`
        // or `languages.use`; Project → only the matching group.
        let mandatory_applies = match entry.manifest.scope {
            GroupScope::Global => true,
            GroupScope::Shared => match project_cfg.as_ref() {
                Some(cfg) => is_group_adopted(&entry.manifest.slug, cfg),
                None => false,
            },
            GroupScope::Project => is_project,
        };
        let files = list_all_memory_files(&backend, &entry.handle, &Rev::head())
            .await
            .context("listing memory files")?;
        for file_ref in &files {
            let Ok(bytes) = backend
                .read_file(&entry.handle, &file_ref.path, &Rev::head())
                .await
            else {
                continue;
            };
            let Ok(text) = std::str::from_utf8(&bytes) else {
                continue;
            };
            let Ok(file) = MemoryFile::parse(text) else {
                continue;
            };
            let row = Row {
                group: entry.manifest.slug.clone(),
                slug: file_ref.slug.clone(),
                kind: file.frontmatter.kind.as_str().to_string(),
                name: file.frontmatter.name.clone(),
                description: file.frontmatter.description.clone(),
            };
            if want_mandatory && file.frontmatter.mandatory && mandatory_applies {
                mandatory_rows.push(row.clone());
            }
            if want_project && is_project {
                project_rows.push(row);
            }
        }
    }

    if matches!(scope, Scope::Mandatory | Scope::All) {
        print_section("Mandatory", &mandatory_rows);
    }
    if matches!(scope, Scope::Project | Scope::All) {
        print_section("Project", &project_rows);
    }

    if mandatory_rows.is_empty() && project_rows.is_empty() {
        println!("(no memories matched the selected scope)");
    } else {
        println!(
            "\nuse `mmcp memory read <group> <slug>` to fetch any body."
        );
    }
    Ok(())
}

#[derive(Clone)]
struct Row {
    group: String,
    slug: String,
    kind: String,
    name: String,
    description: String,
}

/// FR-025 adoption test: is this Shared-scoped group brought into
/// scope by the current project's `.mmcp.toml`? Mirrors the
/// equivalent helper in `commands::serve` which is private. Tiny
/// enough to inline; planned to hoist into `mmcp_store::config`
/// in the Slice 4 refactor pass.
fn is_group_adopted(slug: &str, cfg: &ProjectConfig) -> bool {
    cfg.subscriptions.groups.iter().any(|s| s == slug)
        || cfg
            .subscriptions
            .languages
            .iter()
            .any(|lang| slug == format!("lang/{lang}"))
}

fn print_section(title: &str, rows: &[Row]) {
    if rows.is_empty() {
        println!("{title}: (none)");
        return;
    }
    println!("{title}:");
    for row in rows {
        println!(
            "  {group}/{slug}  [{kind}]  {name}",
            group = row.group,
            slug = row.slug,
            kind = row.kind,
            name = row.name,
        );
        if !row.description.is_empty() {
            println!("      {}", row.description);
        }
    }
}
