//! D1: one-shot migration that rewrites the `FeatureStatus`
//! lifecycle vocabulary on every on-disk feature memory:
//! `open` -> `requested`, `resolved` -> `completed`. Every other
//! status (`blocked`, `deferred`, `duplicate`, `superseded`) is
//! already correct and is left untouched.
//!
//! Usage:
//!
//! ```text
//! cargo run -p mmcp-client --example migrate_feature_status -- [--dry-run]
//! ```
//!
//! Operates on the raw TOML frontmatter text, not the typed
//! `FeatureMetadata` parser: once the `FeatureStatus` rename lands,
//! the enum no longer accepts `open` / `resolved`, so a typed parse
//! of an unmigrated memory fails before `list_features` /
//! `read_feature` would ever hand it to this binary. That failure
//! is exactly the problem this migration exists to fix, so it walks
//! slug directories and reads/writes raw bytes instead.
//!
//! Idempotent: a memory whose `[feature]` status is already one of
//! the new spellings, one of the untouched side-states, or that
//! carries no `[feature]` table at all (a non-feature memory, or an
//! `[issue]`-only memory -- `IssueStatus` keeps its own distinct
//! `open` variant, D1 does not touch it) is left alone with no
//! commit.

use anyhow::{Context, Result};
use mmcp_git::{GitBackend, Rev};
use mmcp_store::{
    AddressingMode, WriteFileOptions, list_memory_slug_dirs, resolve_memory, write_file_at_path,
};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let dry_run = std::env::args().any(|a| a == "--dry-run");
    if dry_run {
        eprintln!("[migrate_feature_status] --dry-run: no commits will be written");
    }

    let home = mmcp_store::MmcpHome::discover()?;
    let (backend, groups) = home.init_backend().await?;
    let author = home.resolve_author();

    let mut total_migrated = 0usize;
    let mut group_counts: Vec<(String, usize)> = Vec::new();

    for entry in groups.list().await {
        let group_uuid = entry.manifest.group_id.to_string();
        let slug_dirs = list_memory_slug_dirs(&backend, &entry.handle, &Rev::head())
            .await
            .with_context(|| format!("listing memories in group {group_uuid}"))?;

        let mut plans: Vec<MigrationPlan> = Vec::new();
        for slug_dir in &slug_dirs {
            // Slug-only resolution never parses file content (it
            // only lists UUID-named blobs in the slug directory),
            // so it works even on a memory whose typed parse would
            // now fail. Ambiguous or unresolvable slugs are a
            // pre-existing `diagnose` concern, not this migration's;
            // skip them rather than guessing.
            let Ok(resolved) =
                resolve_memory(&backend, &entry.handle, Some(&slug_dir.slug), None).await
            else {
                continue;
            };
            let Ok(bytes) = backend
                .read_file(&entry.handle, &resolved.path, &Rev::head())
                .await
            else {
                continue;
            };
            let text = String::from_utf8_lossy(&bytes).into_owned();
            if let Some((rendered, old_status, new_status)) = migrate_status_in_source(&text) {
                plans.push(MigrationPlan {
                    slug: slug_dir.slug.clone(),
                    path: resolved.path.clone(),
                    old_status,
                    new_status,
                    rendered,
                });
            }
        }

        if plans.is_empty() {
            continue;
        }

        eprintln!(
            "[migrate_feature_status] group {} (slug={})",
            group_uuid, entry.manifest.slug
        );
        for plan in &plans {
            eprintln!(
                "  {}: {} -> {}",
                plan.slug, plan.old_status, plan.new_status
            );
        }

        if dry_run {
            group_counts.push((entry.manifest.slug.clone(), plans.len()));
            continue;
        }

        let migrated_here = plans.len();
        for plan in plans {
            write_file_at_path(
                &backend,
                &entry.handle,
                &plan.path,
                &plan.rendered,
                &author,
                WriteFileOptions {
                    addressing_mode: AddressingMode::ByFilename,
                    force: false,
                    message: Some(&format!(
                        "migrate(D1): {} -> {} on {}",
                        plan.old_status, plan.new_status, plan.slug
                    )),
                },
            )
            .await
            .with_context(|| format!("migrating {} in group {group_uuid}", plan.slug))?;
        }
        total_migrated += migrated_here;
        group_counts.push((entry.manifest.slug.clone(), migrated_here));
    }

    if dry_run {
        let total: usize = group_counts.iter().map(|(_, n)| n).sum();
        eprintln!("[migrate_feature_status] dry run: {total} feature(s) would migrate");
    } else {
        eprintln!("[migrate_feature_status] done: migrated={total_migrated}");
    }
    for (slug, count) in &group_counts {
        eprintln!("  {slug}: {count}");
    }
    Ok(())
}

/// What the migration plans to write for a single feature memory.
struct MigrationPlan {
    slug: String,
    path: String,
    old_status: &'static str,
    new_status: &'static str,
    rendered: String,
}

/// Old -> new mapping for the D1 status rename. Every other status
/// (`blocked`, `deferred`, `duplicate`, `superseded`) already reads
/// correctly and has no entry here.
const STATUS_MAP: &[(&str, &str)] = &[("open", "requested"), ("resolved", "completed")];

/// Rewrite the `[feature].status` value in a raw `+++`-fenced TOML
/// memory file from its old D1 spelling to the new one.
///
/// Scoped to the `[feature]` table specifically, never a bare
/// substring search for `status = "open"`: a hybrid memory can carry
/// both `[feature]` and `[issue]` tables, and `IssueStatus` keeps its
/// own unrenamed `open` variant, so a blind replace would corrupt an
/// unrelated table.
///
/// Returns `None` -- the idempotency path -- when the file is not
/// `+++`-fenced TOML, has no `[feature]` table, or its status is
/// already outside the mapping table (new spelling, or an untouched
/// side-state).
fn migrate_status_in_source(source: &str) -> Option<(String, &'static str, &'static str)> {
    let mut lines: Vec<String> = source.split('\n').map(str::to_string).collect();

    if lines.first().map(String::as_str) != Some("+++") {
        return None;
    }
    let close_idx = lines[1..].iter().position(|l| l == "+++")? + 1;

    let feature_idx = lines[1..close_idx]
        .iter()
        .position(|l| l.trim() == "[feature]")
        .map(|i| i + 1)?;

    // The table runs until the next `[...]` header or the closing
    // fence, whichever comes first.
    let table_end = lines[feature_idx + 1..close_idx]
        .iter()
        .position(|l| l.trim_start().starts_with('['))
        .map(|i| feature_idx + 1 + i)
        .unwrap_or(close_idx);

    for idx in feature_idx..table_end {
        let trimmed = lines[idx].trim();
        let Some(rest) = trimmed.strip_prefix("status") else {
            continue;
        };
        let Some(value) = rest.trim_start().strip_prefix('=') else {
            continue;
        };
        let value = value.trim();
        for (old, new) in STATUS_MAP {
            if value == format!("\"{old}\"") {
                let indent_len = lines[idx].len() - lines[idx].trim_start().len();
                let indent = lines[idx][..indent_len].to_string();
                lines[idx] = format!("{indent}status = \"{new}\"");
                return Some((lines.join("\n"), old, new));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source_with_status(status: &str) -> String {
        format!(
            "+++\nid = \"019fccb3-f40d-7843-b52f-b211ec496f17\"\nname = \"x\"\ndescription = \"y\"\nkind = \"feature\"\nmandatory = false\ntags = []\n\n[feature]\nstatus = \"{status}\"\nnumber = 1\n+++\n## Need\n\nbody\n"
        )
    }

    #[test]
    fn migrates_open_to_requested() {
        let source = source_with_status("open");
        let (rendered, old, new) = migrate_status_in_source(&source).expect("migration");
        assert_eq!(old, "open");
        assert_eq!(new, "requested");
        assert!(rendered.contains("status = \"requested\""));
        assert!(!rendered.contains("status = \"open\""));
        // Every other line survives untouched.
        assert!(rendered.contains("number = 1"));
        assert!(rendered.contains("## Need"));
    }

    #[test]
    fn migrates_resolved_to_completed() {
        let source = source_with_status("resolved");
        let (rendered, old, new) = migrate_status_in_source(&source).expect("migration");
        assert_eq!(old, "resolved");
        assert_eq!(new, "completed");
        assert!(rendered.contains("status = \"completed\""));
    }

    #[test]
    fn leaves_side_states_unchanged() {
        for status in ["blocked", "deferred", "duplicate", "superseded"] {
            let source = source_with_status(status);
            assert!(
                migrate_status_in_source(&source).is_none(),
                "{status} must not be touched"
            );
        }
    }

    #[test]
    fn is_idempotent_on_already_migrated_statuses() {
        for status in ["requested", "approved", "pending", "completed"] {
            let source = source_with_status(status);
            assert!(
                migrate_status_in_source(&source).is_none(),
                "{status} must already be a no-op"
            );
        }
    }

    #[test]
    fn ignores_memories_without_a_feature_table() {
        let source = "+++\nname = \"x\"\ndescription = \"y\"\nkind = \"rule\"\n+++\nbody\n";
        assert!(migrate_status_in_source(source).is_none());
    }

    #[test]
    fn does_not_touch_an_issue_table_status() {
        // Hybrid memory: `[feature]` migrates, the sibling
        // `[issue]` table's own `open` status (a different
        // vocabulary, D1 leaves it alone) must survive verbatim.
        let source = "+++\nname = \"x\"\ndescription = \"y\"\nkind = \"feature\"\n\n[feature]\nstatus = \"open\"\n\n[issue]\nstatus = \"open\"\n+++\nbody\n";
        let (rendered, old, new) = migrate_status_in_source(source).expect("migration");
        assert_eq!((old, new), ("open", "requested"));
        assert!(rendered.contains("[feature]\nstatus = \"requested\""));
        assert!(rendered.contains("[issue]\nstatus = \"open\""));
    }
}
