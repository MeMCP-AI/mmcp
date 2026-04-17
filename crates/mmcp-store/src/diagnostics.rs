//! Health (surface) and diagnostic (deep) checks for memory repos.
//!
//! - **Health**: quick pass/fail per group. Is the manifest valid?
//!   Do all memory files parse? Returns counts and errors only.
//! - **Diagnose**: deep analysis. Reports missing optional fields,
//!   naming drift, empty groups, semver parse issues, directory/
//!   manifest UUID mismatches, and structural hints.
//!
//! Neither endpoint ever modifies data. Consumers (CLI check /
//! diagnose, MCP `check_health` / `diagnose` tools, GUI diagnostic
//! widgets) receive typed `GroupReport` / `DiagReport` structs and
//! decide how to render them.
//!
//! History: ported from `crates/mmcp-client/src/commands/health.rs`
//! during the FR-020 extraction. The CLI runners
//! (`run_check`, `run_diagnose`, `print_reports`) stay in the
//! client crate because they carry exit-code + stdout shaping that
//! belong in the binary.

use mmcp_core::conventions::{MEMORIES_DIR, MEMORY_EXTENSION};
use mmcp_core::manifest::MANIFEST_SCHEMA_VERSION;
use mmcp_core::memory::MemoryFile;
use mmcp_git::{GitBackend, NativeBackend, Rev};
use serde::Serialize;

use crate::groups::{GroupEntry, GroupIndex};
use crate::home::{MmcpHome, read_git_global};
use crate::memory::slugify_filename;

// ── Shared types ────────────────────────────────────────────

/// One issue found during check.
#[derive(Debug, Clone, Serialize)]
pub struct Issue {
    pub group: String,
    pub slug: Option<String>,
    pub severity: &'static str,
    pub message: String,
}

/// Report for a single group.
#[derive(Debug, Clone, Serialize)]
pub struct GroupReport {
    pub group_id: String,
    pub slug: String,
    pub manifest_ok: bool,
    pub memory_count: usize,
    pub issues: Vec<Issue>,
}

/// Full diagnostic report including project-level issues.
#[derive(Debug, Clone, Serialize)]
pub struct DiagReport {
    pub project_issues: Vec<Issue>,
    pub groups: Vec<GroupReport>,
}

// ── Health (surface) ────────────────────────────────────────

/// Quick surface check: manifest parseable, all memories parse,
/// required fields present. No hints, no deep analysis.
pub async fn health_check_group(backend: &NativeBackend, entry: &GroupEntry) -> GroupReport {
    let gid = entry.manifest.group_id.as_uuid().to_string();
    let slug = entry.manifest.slug.clone();
    let mut issues = Vec::new();

    // Manifest
    let manifest_ok = match backend.read_manifest(&entry.handle).await {
        Ok(m) => {
            if m.schema_version > MANIFEST_SCHEMA_VERSION {
                issues.push(Issue {
                    group: gid.clone(),
                    slug: None,
                    severity: "error",
                    message: format!(
                        "manifest schema_version {} is newer than supported {}",
                        m.schema_version, MANIFEST_SCHEMA_VERSION
                    ),
                });
            }
            if m.slug.is_empty() {
                issues.push(Issue {
                    group: gid.clone(),
                    slug: None,
                    severity: "error",
                    message: "manifest slug is empty".to_string(),
                });
            }
            true
        }
        Err(err) => {
            issues.push(Issue {
                group: gid.clone(),
                slug: None,
                severity: "error",
                message: format!("manifest unreadable: {err}"),
            });
            false
        }
    };

    // Memories
    let rev = Rev::head();
    let files = match backend.list_tree(&entry.handle, MEMORIES_DIR, &rev).await {
        Ok(f) => f,
        Err(err) => {
            issues.push(Issue {
                group: gid.clone(),
                slug: None,
                severity: "warning",
                message: format!("cannot list memories/: {err}"),
            });
            return GroupReport {
                group_id: gid,
                slug,
                manifest_ok,
                memory_count: 0,
                issues,
            };
        }
    };

    let mut memory_count = 0;
    for filename in &files {
        let Some(mem_slug) = filename.strip_suffix(MEMORY_EXTENSION) else {
            issues.push(Issue {
                group: gid.clone(),
                slug: Some(filename.clone()),
                severity: "warning",
                message: format!("non-{MEMORY_EXTENSION} file in {MEMORIES_DIR}/"),
            });
            continue;
        };
        memory_count += 1;

        let path = format!("{MEMORIES_DIR}/{filename}");
        match backend.read_file(&entry.handle, &path, &rev).await {
            Ok(bytes) => {
                let Ok(text) = std::str::from_utf8(&bytes) else {
                    issues.push(Issue {
                        group: gid.clone(),
                        slug: Some(mem_slug.to_string()),
                        severity: "error",
                        message: "not valid UTF-8".to_string(),
                    });
                    continue;
                };
                if let Err(err) = MemoryFile::parse(text) {
                    issues.push(Issue {
                        group: gid.clone(),
                        slug: Some(mem_slug.to_string()),
                        severity: "error",
                        message: format!("frontmatter parse failed: {err}"),
                    });
                }
            }
            Err(err) => {
                issues.push(Issue {
                    group: gid.clone(),
                    slug: Some(mem_slug.to_string()),
                    severity: "error",
                    message: format!("cannot read: {err}"),
                });
            }
        }
    }

    GroupReport {
        group_id: gid,
        slug,
        manifest_ok,
        memory_count,
        issues,
    }
}

pub async fn health_check_all(backend: &NativeBackend, groups: &GroupIndex) -> Vec<GroupReport> {
    let entries = groups.list().await;
    let mut reports = Vec::with_capacity(entries.len());
    for entry in entries {
        reports.push(health_check_group(backend, &entry).await);
    }
    reports
}

// ── Diagnose (deep) ─────────────────────────────────────────

/// Deep diagnostic analysis. Runs everything health does plus:
/// - Missing optional fields (tags, version) as info hints
/// - Empty body detection
/// - Empty group (zero memories)
/// - Manifest group_id vs directory UUID mismatch
/// - Manifest created_at sanity
/// - Slug/name drift (filename slug != frontmatter name)
/// - Semver parse issues in version field
/// - Duplicate slugs across groups
pub async fn diagnose_group(backend: &NativeBackend, entry: &GroupEntry) -> GroupReport {
    let mut report = health_check_group(backend, entry).await;
    let gid = report.group_id.clone();
    let rev = Rev::head();

    // Deep manifest checks
    if let Ok(m) = backend.read_manifest(&entry.handle).await {
        // group_id vs directory UUID
        let dir_uuid = entry.handle.group_id.to_string();
        let manifest_uuid = m.group_id.as_uuid().to_string();
        if dir_uuid != manifest_uuid {
            report.issues.push(Issue {
                group: gid.clone(),
                slug: None,
                severity: "error",
                message: format!(
                    "manifest group_id {manifest_uuid} does not match directory UUID {dir_uuid}"
                ),
            });
        }

        // created_at sanity
        if m.created_at <= 0 {
            report.issues.push(Issue {
                group: gid.clone(),
                slug: None,
                severity: "warning",
                message: format!(
                    "manifest created_at is {}, expected positive timestamp",
                    m.created_at
                ),
            });
        }

        // display_name hint
        if m.display_name.is_none() {
            report.issues.push(Issue {
                group: gid.clone(),
                slug: None,
                severity: "info",
                message: "manifest has no display_name set".to_string(),
            });
        }
    }

    // Empty group
    if report.memory_count == 0 {
        report.issues.push(Issue {
            group: gid.clone(),
            slug: None,
            severity: "info",
            message: "group has zero memories".to_string(),
        });
    }

    // Deep per-memory checks
    let files = backend
        .list_tree(&entry.handle, MEMORIES_DIR, &rev)
        .await
        .unwrap_or_default();

    for filename in &files {
        let Some(mem_slug) = filename.strip_suffix(MEMORY_EXTENSION) else {
            continue;
        };
        let path = format!("{MEMORIES_DIR}/{filename}");
        let Ok(bytes) = backend.read_file(&entry.handle, &path, &rev).await else {
            continue;
        };
        let Ok(text) = std::str::from_utf8(&bytes) else {
            continue;
        };
        let Ok(file) = MemoryFile::parse(text) else {
            continue; // Already reported by health
        };
        let fm = &file.frontmatter;

        // Required field quality
        if fm.name.is_empty() {
            report.issues.push(Issue {
                group: gid.clone(),
                slug: Some(mem_slug.to_string()),
                severity: "error",
                message: "name is empty".to_string(),
            });
        }
        if fm.description.is_empty() {
            report.issues.push(Issue {
                group: gid.clone(),
                slug: Some(mem_slug.to_string()),
                severity: "error",
                message: "description is empty".to_string(),
            });
        }

        // Body quality
        if file.body.trim().is_empty() {
            report.issues.push(Issue {
                group: gid.clone(),
                slug: Some(mem_slug.to_string()),
                severity: "warning",
                message: "body is empty".to_string(),
            });
        }

        // Tags hint
        if fm.tags.is_empty() {
            report.issues.push(Issue {
                group: gid.clone(),
                slug: Some(mem_slug.to_string()),
                severity: "info",
                message: "no tags set (reduces discoverability)".to_string(),
            });
        }

        // Version field parse check
        if let Some(ref v) = fm.version {
            // Already parsed as semver by serde, but check it's reasonable
            if v.major == 0 && v.minor == 0 && v.patch == 0 {
                report.issues.push(Issue {
                    group: gid.clone(),
                    slug: Some(mem_slug.to_string()),
                    severity: "info",
                    message: "version is 0.0.0 (not yet published?)".to_string(),
                });
            }
        }

        // Slug/name drift: is the frontmatter name related to the slug?
        let name_slug = slugify_filename(&fm.name);
        if !name_slug.is_empty()
            && name_slug != mem_slug
            && !mem_slug.contains(&name_slug)
            && !name_slug.contains(mem_slug)
        {
            report.issues.push(Issue {
                group: gid.clone(),
                slug: Some(mem_slug.to_string()),
                severity: "info",
                message: format!(
                    "slug '{mem_slug}' and name '{}' may have drifted (slugified name: '{name_slug}')",
                    fm.name
                ),
            });
        }
    }

    report
}

pub async fn diagnose_all(backend: &NativeBackend, groups: &GroupIndex) -> DiagReport {
    let mut project_issues = Vec::new();

    // Project-level: check if a sync server is configured
    check_project_config(&mut project_issues);

    let entries = groups.list().await;
    let mut reports = Vec::with_capacity(entries.len());

    // Collect all slugs for cross-group duplicate detection
    let mut slug_groups: std::collections::HashMap<String, Vec<String>> =
        std::collections::HashMap::new();

    for entry in &entries {
        let report = diagnose_group(backend, entry).await;
        let gid = report.group_id.clone();
        let rev = Rev::head();
        if let Ok(files) = backend.list_tree(&entry.handle, MEMORIES_DIR, &rev).await {
            for f in files {
                if let Some(s) = f.strip_suffix(MEMORY_EXTENSION) {
                    slug_groups
                        .entry(s.to_string())
                        .or_default()
                        .push(gid.clone());
                }
            }
        }
        reports.push(report);
    }

    // Cross-group slug duplicates
    for (dup_slug, group_ids) in &slug_groups {
        if group_ids.len() > 1 {
            for report in &mut reports {
                if group_ids.contains(&report.group_id) {
                    report.issues.push(Issue {
                        group: report.group_id.clone(),
                        slug: Some(dup_slug.clone()),
                        severity: "info",
                        message: format!(
                            "slug '{dup_slug}' also exists in {} other group(s)",
                            group_ids.len() - 1
                        ),
                    });
                }
            }
        }
    }

    DiagReport {
        project_issues,
        groups: reports,
    }
}

/// Check project-level and user-level configuration for issues.
fn check_project_config(issues: &mut Vec<Issue>) {
    // User-level config
    let home = MmcpHome::discover().ok();
    let user_cfg = home
        .as_ref()
        .and_then(|h| h.load_user_config().ok())
        .unwrap_or_default();

    if home.as_ref().is_none_or(|h| !h.user_config_path().exists()) {
        issues.push(Issue {
            group: "(user)".to_string(),
            slug: None,
            severity: "warning",
            message: "no user config at ~/.mmcp/config.toml - create one to set author identity and default sync server".to_string(),
        });
    } else {
        // Check author config
        match user_cfg.author.as_ref() {
            None => {
                issues.push(Issue {
                    group: "(user)".to_string(),
                    slug: None,
                    severity: "warning",
                    message: "[author] section missing in user config".to_string(),
                });
            }
            Some(author) => {
                match author.git_fallback {
                    None => {
                        issues.push(Issue {
                            group: "(user)".to_string(),
                            slug: None,
                            severity: "warning",
                            message: "author.git_fallback not set - set to true (use git identity) or false (use mmcp fallback)".to_string(),
                        });
                    }
                    Some(true) => {
                        // Check if git config actually has a name when no override is set.
                        // Uses the same gix-based reader as author resolution so the two
                        // views can never disagree.
                        if author.name.is_none() && read_git_global("user.name").is_none() {
                            issues.push(Issue {
                                group: "(user)".to_string(),
                                slug: None,
                                severity: "warning",
                                message: "git_fallback=true but git config user.name is empty"
                                    .to_string(),
                            });
                        }
                    }
                    Some(false) => {
                        // Explicit opt-out: respected silently, no warning
                    }
                }
            }
        }
    }

    // Check sync: user config OR project config must have it
    let user_has_sync = user_cfg.sync.is_some();

    // Project-level config
    let cwd = match std::env::current_dir() {
        Ok(d) => d,
        Err(_) => {
            if !user_has_sync {
                issues.push(Issue {
                    group: "(project)".to_string(),
                    slug: None,
                    severity: "warning",
                    message: "no [sync] server configured in user or project config".to_string(),
                });
            }
            return;
        }
    };

    match crate::config::find_project_root(&cwd) {
        None => {
            issues.push(Issue {
                group: "(project)".to_string(),
                slug: None,
                severity: "warning",
                message: "no .mmcp.toml project config found in current directory or any parent"
                    .to_string(),
            });
            if !user_has_sync {
                issues.push(Issue {
                    group: "(project)".to_string(),
                    slug: None,
                    severity: "warning",
                    message: "no [sync] server configured anywhere - push/pull will not work"
                        .to_string(),
                });
            }
        }
        Some(root) => match crate::config::load(&root) {
            Err(err) => {
                issues.push(Issue {
                    group: "(project)".to_string(),
                    slug: None,
                    severity: "error",
                    message: format!("project config failed to load: {err}"),
                });
            }
            Ok(cfg) => {
                if cfg.sync.is_none() && !user_has_sync {
                    issues.push(Issue {
                        group: "(project)".to_string(),
                        slug: None,
                        severity: "warning",
                        message: "no [sync] server configured in user or project config - push/pull will not work".to_string(),
                    });
                }
            }
        },
    }
}
