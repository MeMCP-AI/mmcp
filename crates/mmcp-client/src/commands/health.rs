//! Memory health check - validates manifests and memory files.
//!
//! Shared logic for both the `mmcp check` CLI subcommand and
//! the `check_health` MCP tool.

use anyhow::Result;
use mmcp_core::conventions::{MEMORIES_DIR, MEMORY_EXTENSION};
use mmcp_core::memory::MemoryFile;
use mmcp_git::{GitBackend, NativeBackend, Rev};
use serde::Serialize;

use crate::state::{GroupEntry, GroupIndex};

// GroupManifest is used for validation
use mmcp_core::manifest::GroupManifest;

/// One issue found during health check.
#[derive(Debug, Clone, Serialize)]
pub struct HealthIssue {
    pub group: String,
    pub slug: Option<String>,
    pub severity: &'static str,
    pub message: String,
}

/// Full health report for a group.
#[derive(Debug, Clone, Serialize)]
pub struct GroupHealthReport {
    pub group_id: String,
    pub slug: String,
    pub manifest_ok: bool,
    pub memory_count: usize,
    pub issues: Vec<HealthIssue>,
}

/// Check the health of a single group.
pub async fn check_group(
    backend: &NativeBackend,
    entry: &GroupEntry,
) -> GroupHealthReport {
    let group_id = entry.manifest.group_id.as_uuid().to_string();
    let slug = entry.manifest.slug.clone();
    let mut issues = Vec::new();

    // 1. Validate manifest
    let manifest_ok = match backend
        .read_manifest(&entry.handle)
        .await
    {
        Ok(manifest) => {
            validate_manifest(&manifest, &group_id, &slug, &mut issues);
            true
        }
        Err(err) => {
            issues.push(HealthIssue {
                group: group_id.clone(),
                slug: None,
                severity: "error",
                message: format!("manifest unreadable: {err}"),
            });
            false
        }
    };

    // 2. List and validate all memory files
    let rev = Rev::Branch(mmcp_core::conventions::MAIN_BRANCH.to_string());
    let memory_files = match backend
        .list_tree(&entry.handle, MEMORIES_DIR, &rev)
        .await
    {
        Ok(files) => files,
        Err(err) => {
            issues.push(HealthIssue {
                group: group_id.clone(),
                slug: None,
                severity: "warning",
                message: format!("cannot list memories directory: {err}"),
            });
            return GroupHealthReport {
                group_id,
                slug,
                manifest_ok,
                memory_count: 0,
                issues,
            };
        }
    };

    let mut memory_count = 0;
    for filename in &memory_files {
        let Some(mem_slug) = filename.strip_suffix(MEMORY_EXTENSION) else {
            issues.push(HealthIssue {
                group: group_id.clone(),
                slug: Some(filename.clone()),
                severity: "warning",
                message: format!("non-.md file in memories/: {filename}"),
            });
            continue;
        };
        memory_count += 1;

        let path = format!("{MEMORIES_DIR}/{filename}");
        match backend.read_file(&entry.handle, &path, &rev).await {
            Ok(bytes) => {
                let text = match std::str::from_utf8(&bytes) {
                    Ok(t) => t,
                    Err(err) => {
                        issues.push(HealthIssue {
                            group: group_id.clone(),
                            slug: Some(mem_slug.to_string()),
                            severity: "error",
                            message: format!("not valid UTF-8: {err}"),
                        });
                        continue;
                    }
                };
                match MemoryFile::parse(text) {
                    Ok(file) => {
                        validate_memory(&file, mem_slug, &group_id, &mut issues);
                    }
                    Err(err) => {
                        issues.push(HealthIssue {
                            group: group_id.clone(),
                            slug: Some(mem_slug.to_string()),
                            severity: "error",
                            message: format!("frontmatter parse failed: {err}"),
                        });
                    }
                }
            }
            Err(err) => {
                issues.push(HealthIssue {
                    group: group_id.clone(),
                    slug: Some(mem_slug.to_string()),
                    severity: "error",
                    message: format!("cannot read file: {err}"),
                });
            }
        }
    }

    GroupHealthReport {
        group_id,
        slug,
        manifest_ok,
        memory_count,
        issues,
    }
}

fn validate_manifest(
    manifest: &GroupManifest,
    group_id: &str,
    _slug: &str,
    issues: &mut Vec<HealthIssue>,
) {
    if manifest.slug.is_empty() {
        issues.push(HealthIssue {
            group: group_id.to_string(),
            slug: None,
            severity: "error",
            message: "manifest slug is empty".to_string(),
        });
    }
    if manifest.schema_version != 1 {
        issues.push(HealthIssue {
            group: group_id.to_string(),
            slug: None,
            severity: "warning",
            message: format!(
                "manifest schema_version is {} (expected 1)",
                manifest.schema_version
            ),
        });
    }
}

fn validate_memory(
    file: &MemoryFile,
    mem_slug: &str,
    group_id: &str,
    issues: &mut Vec<HealthIssue>,
) {
    if file.frontmatter.name.is_empty() {
        issues.push(HealthIssue {
            group: group_id.to_string(),
            slug: Some(mem_slug.to_string()),
            severity: "error",
            message: "frontmatter name is empty".to_string(),
        });
    }
    if file.frontmatter.description.is_empty() {
        issues.push(HealthIssue {
            group: group_id.to_string(),
            slug: Some(mem_slug.to_string()),
            severity: "error",
            message: "frontmatter description is empty".to_string(),
        });
    }
    if file.body.trim().is_empty() {
        issues.push(HealthIssue {
            group: group_id.to_string(),
            slug: Some(mem_slug.to_string()),
            severity: "warning",
            message: "memory body is empty".to_string(),
        });
    }
}

/// Check all groups.
pub async fn check_all(
    backend: &NativeBackend,
    groups: &GroupIndex,
) -> Vec<GroupHealthReport> {
    let entries = groups.list().await;
    let mut reports = Vec::with_capacity(entries.len());
    for entry in entries {
        reports.push(check_group(backend, &entry).await);
    }
    reports
}

// ── CLI entry point ─────────────────────────────────────────

/// CLI entry point for `mmcp check`.
pub async fn run(group: Option<String>) -> Result<()> {
    let mmcp_home = crate::home::MmcpHome::discover()?;
    let (backend, group_index) = mmcp_home.init_backend().await?;

    let reports = if let Some(id) = group {
        let entry = crate::commands::import::resolve_group(&group_index, &id)
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        vec![check_group(&backend, &entry).await]
    } else {
        check_all(&backend, &group_index).await
    };

    let mut total_issues = 0;
    for report in &reports {
        if report.issues.is_empty() {
            println!(
                "{} ({}): {} memories, no issues",
                report.slug, report.group_id, report.memory_count
            );
        } else {
            println!(
                "{} ({}): {} memories, {} issues:",
                report.slug,
                report.group_id,
                report.memory_count,
                report.issues.len()
            );
            for issue in &report.issues {
                let target = issue
                    .slug
                    .as_deref()
                    .unwrap_or("manifest");
                println!(
                    "  [{}] {}: {}",
                    issue.severity, target, issue.message
                );
            }
            total_issues += report.issues.len();
        }
    }

    if total_issues > 0 {
        println!("\n{total_issues} issue(s) found");
        std::process::exit(1);
    } else {
        println!("\nall healthy");
    }
    Ok(())
}
