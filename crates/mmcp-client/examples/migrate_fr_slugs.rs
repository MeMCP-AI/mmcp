//! FR-027: one-shot migration that strips the historical
//! `fr-NNN-` prefix from every feature memory's slug and
//! stamps the parsed number into the typed
//! `feature.number` metadata field.
//!
//! Usage:
//!
//! ```text
//! cargo run -p mmcp-client --example migrate_fr_slugs -- [--dry-run]
//! ```
//!
//! Walks every group repo under `~/.mmcp/repos/`, enumerates
//! `kind ∈ {fr, feature}` memories, and rewrites each slug that
//! starts with `fr-NNN-` to the `<rest>` portion while preserving
//! the sequence number as `feature.number`. The slug rename goes
//! through `rename_feature` (atomic) and the number stamp through
//! `update_feature`; both reuse the same store-layer primitives
//! the MCP surface calls so the two paths can never diverge.
//!
//! Idempotent: features whose slug already lacks the `fr-NNN-`
//! prefix or whose metadata already carries a `number` are
//! skipped with no commit. Runs post-FR-028: assumes every
//! feature lives at `memories/<slug>/<uuid>.md`.

use anyhow::{Context, Result};
use mmcp_core::memory::FeatureStatus;
use mmcp_store::features::{FeatureRecord, UpdateSpec, list_features, rename_feature, update_feature};
use mmcp_store::home::MmcpHome;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let dry_run = std::env::args().any(|a| a == "--dry-run");
    if dry_run {
        eprintln!("[migrate_fr_slugs] --dry-run: no commits will be written");
    }

    let home = MmcpHome::discover()?;
    let (backend, groups) = home.init_backend().await?;
    let author = home.resolve_author();

    let mut total_renamed = 0usize;
    let mut total_numbered = 0usize;
    let mut total_skipped = 0usize;

    for entry in groups.list().await {
        let group_uuid = entry.manifest.group_id.to_string();
        eprintln!(
            "[migrate_fr_slugs] group {} (slug={})",
            group_uuid, entry.manifest.slug
        );

        // `list_features(show_all=true)` returns every feature
        // regardless of status so resolved / blocked FRs are
        // migrated too.
        let records = list_features(&backend, &entry, None, true)
            .await
            .with_context(|| format!("listing features in group {group_uuid}"))?;
        if records.is_empty() {
            continue;
        }

        // Collect the planned work before mutating anything so a
        // dry run preview lists every migration unconditionally.
        let mut plans: Vec<MigrationPlan> = Vec::new();
        for record in records {
            if let Some(plan) = plan_migration(&record) {
                plans.push(plan);
            } else {
                total_skipped += 1;
            }
        }

        for plan in plans {
            eprintln!(
                "  {} -> {} (number={})",
                plan.old_slug, plan.new_slug, plan.number
            );
            if dry_run {
                continue;
            }

            // Rename the slug directory first. Every UUID file
            // under `memories/<old_slug>/` moves in one atomic
            // commit; cross-references pointing at UUIDs stay
            // valid automatically.
            if plan.old_slug != plan.new_slug {
                rename_feature(
                    &backend,
                    &entry,
                    &plan.old_slug,
                    &plan.new_slug,
                    &author,
                    Some(&format!(
                        "migrate(FR-027): strip fr-NNN prefix from {}",
                        plan.old_slug
                    )),
                )
                .await
                .with_context(|| {
                    format!(
                        "renaming {} to {} in group {group_uuid}",
                        plan.old_slug, plan.new_slug
                    )
                })?;
                total_renamed += 1;
            }

            // Stamp the parsed number. `update_feature` preserves
            // every other field when only `number` is supplied.
            if plan.needs_number_stamp {
                update_feature(
                    &backend,
                    &entry,
                    &plan.new_slug,
                    UpdateSpec {
                        number: Some(plan.number),
                        message: Some(format!(
                            "migrate(FR-027): stamp number={} on {}",
                            plan.number, plan.new_slug
                        )),
                        ..UpdateSpec::default()
                    },
                    &author,
                )
                .await
                .with_context(|| {
                    format!("stamping number on {} in group {group_uuid}", plan.new_slug)
                })?;
                total_numbered += 1;
            }
        }
    }

    eprintln!(
        "[migrate_fr_slugs] done: renamed={total_renamed} numbered={total_numbered} skipped={total_skipped}"
    );
    let _ = FeatureStatus::Open; // keep the import threaded when body evolves
    Ok(())
}

/// What the migration plans to do for a single feature. Built
/// up-front so the dry-run log and the real run see the exact
/// same work plan.
struct MigrationPlan {
    old_slug: String,
    new_slug: String,
    number: u32,
    needs_number_stamp: bool,
}

/// Inspect a feature record and decide whether it needs migrating.
/// Returns `None` when the slug already lacks the `fr-NNN-` prefix
/// and a `number` is already stamped — the idempotency path.
fn plan_migration(record: &FeatureRecord) -> Option<MigrationPlan> {
    let parsed = parse_fr_prefix(&record.slug);
    match (parsed, record.number) {
        // Slug has `fr-NNN-<rest>` and no stamped number -> full
        // migration: rename and stamp.
        (Some((number, rest)), None) => Some(MigrationPlan {
            old_slug: record.slug.clone(),
            new_slug: rest,
            number,
            needs_number_stamp: true,
        }),
        // Slug has `fr-NNN-<rest>` but number is already stamped
        // (probably from the FR-028 migration run picking it up) ->
        // just rename.
        (Some((_, rest)), Some(_)) => Some(MigrationPlan {
            old_slug: record.slug.clone(),
            new_slug: rest,
            number: record.number.expect("guarded by match arm"),
            needs_number_stamp: false,
        }),
        // Slug already clean but number missing -> stamp only. No
        // parseable number available, skip with no plan.
        (None, None) => None,
        // Fully migrated.
        (None, Some(_)) => None,
    }
}

/// Parse `fr-NNN-<rest>` → `(NNN, <rest>)`. Returns `None` for
/// slugs that don't match the prefix shape so arbitrary feature
/// slugs are left alone.
fn parse_fr_prefix(slug: &str) -> Option<(u32, String)> {
    let rest = slug.strip_prefix("fr-")?;
    let (number_str, tail) = rest.split_once('-')?;
    let number = number_str.parse::<u32>().ok()?;
    if tail.is_empty() {
        return None;
    }
    Some((number, tail.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_fr_prefix_extracts_number_and_rest() {
        let parsed = parse_fr_prefix("fr-007-feature-request-tracking");
        assert_eq!(parsed, Some((7, "feature-request-tracking".into())));
    }

    #[test]
    fn parse_fr_prefix_handles_double_digits() {
        let parsed = parse_fr_prefix("fr-028-uuidify-memories-allow-duplicate-slugs");
        assert_eq!(
            parsed,
            Some((28, "uuidify-memories-allow-duplicate-slugs".into()))
        );
    }

    #[test]
    fn parse_fr_prefix_rejects_non_fr_slugs() {
        assert_eq!(parse_fr_prefix("frontmatter-only-read"), None);
        assert_eq!(parse_fr_prefix("global-git-conventions"), None);
        assert_eq!(parse_fr_prefix("fr-notanumber-foo"), None);
        assert_eq!(parse_fr_prefix("fr-007-"), None);
    }

    fn record_with(slug: &str, number: Option<u32>) -> FeatureRecord {
        FeatureRecord {
            slug: slug.to_string(),
            title: String::new(),
            description: String::new(),
            body: String::new(),
            status: FeatureStatus::Open,
            number,
            depends_on: Vec::new(),
            blocks: Vec::new(),
            commit_id: String::new(),
        }
    }

    #[test]
    fn plan_skips_fully_migrated_records() {
        let rec = record_with("feature-request-tracking", Some(7));
        assert!(plan_migration(&rec).is_none());
    }

    #[test]
    fn plan_full_migration_for_legacy_slug_without_number() {
        let rec = record_with("fr-007-feature-request-tracking", None);
        let plan = plan_migration(&rec).expect("plan");
        assert_eq!(plan.old_slug, "fr-007-feature-request-tracking");
        assert_eq!(plan.new_slug, "feature-request-tracking");
        assert_eq!(plan.number, 7);
        assert!(plan.needs_number_stamp);
    }

    #[test]
    fn plan_rename_only_when_number_already_stamped() {
        let rec = record_with("fr-007-feature-request-tracking", Some(7));
        let plan = plan_migration(&rec).expect("plan");
        assert_eq!(plan.new_slug, "feature-request-tracking");
        assert!(!plan.needs_number_stamp);
    }
}
