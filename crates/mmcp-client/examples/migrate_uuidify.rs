//! FR-028: one-shot migration that moves every memory from the
//! flat `memories/<slug>.md` layout to the two-level
//! `memories/<slug>/<uuid>.md` layout, stamps a UUIDv7 into each
//! file's frontmatter, and rewrites FR cross-references from slug
//! strings to UUID strings.
//!
//! Usage:
//!
//! ```text
//! cargo run -p mmcp-client --example migrate_uuidify -- [--dry-run]
//! ```
//!
//! The binary walks every group repo under `~/.mmcp/repos/`, so
//! running it once covers the user's entire mirror. It is idempotent:
//! a memory that already carries an `id` in frontmatter is left
//! alone. This is a one-shot tool; after every memory carries a
//! UUID, the binary can be retired.
//!
//! Design notes:
//! - Works directly on raw frontmatter via `gray_matter` + `toml`
//!   so existing files with legacy slug-based `depends_on` /
//!   `blocks` still parse. The typed `MemoryFrontmatter` deserializer
//!   now requires UUIDs; reading through it before migration would
//!   fail on FR memories with slug cross-refs.
//! - Two passes. Pass 1 moves every memory, mints UUIDs, and
//!   builds a `HashMap<slug, Uuid>`. Pass 2 revisits every FR
//!   memory and rewrites cross-refs using the map.
//! - Every move is an atomic git commit via
//!   `NativeBackend::write_commit`: the new path appears and the
//!   old path disappears in the same commit so `git log` never
//!   shows a broken intermediate state.

use std::collections::HashMap;

use anyhow::{Context, Result, anyhow};
use mmcp_core::conventions::{MEMORIES_DIR, MEMORY_EXTENSION, legacy_memory_path, memory_path};
use mmcp_git::{CommitSpec, GitBackend, NativeBackend, RepoHandle, Rev};
use mmcp_store::home::{MmcpHome, ResolvedAuthor};
use uuid::Uuid;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let dry_run = std::env::args().any(|a| a == "--dry-run");
    if dry_run {
        eprintln!("[migrate_uuidify] --dry-run: no commits will be written");
    }

    let home = MmcpHome::discover()?;
    let (backend, groups) = home.init_backend().await?;
    let author = home.resolve_author();

    let mut slug_to_id: HashMap<String, Uuid> = HashMap::new();
    let mut total_migrated = 0usize;
    let mut total_skipped = 0usize;
    let mut fr_candidates: Vec<(String, String, Uuid)> = Vec::new(); // (group_uuid, slug, id)

    // ── Pass 1: move every legacy-layout memory, mint UUIDs ────────
    for entry in groups.list().await {
        let group_uuid = entry.manifest.group_id.to_string();
        eprintln!(
            "[migrate_uuidify] group {} (slug={})",
            group_uuid, entry.manifest.slug
        );

        let legacy_files = backend
            .list_tree(&entry.handle, MEMORIES_DIR, &Rev::head())
            .await
            .with_context(|| format!("listing memories in group {group_uuid}"))?;

        for file_name in legacy_files {
            // `list_tree` returns blobs only, so subtrees from a
            // partially-migrated layout are invisible here. The
            // flat `.md` entries are the migration targets.
            let Some(slug) = file_name.strip_suffix(MEMORY_EXTENSION) else {
                continue;
            };
            let legacy_path = legacy_memory_path(slug);

            let bytes = backend
                .read_file(&entry.handle, &legacy_path, &Rev::head())
                .await
                .with_context(|| format!("reading {legacy_path} in group {group_uuid}"))?;
            let text = std::str::from_utf8(&bytes)
                .with_context(|| format!("{legacy_path} is not valid UTF-8"))?;

            let split = split_frontmatter(text).with_context(|| {
                format!("parsing frontmatter of {legacy_path} in group {group_uuid}")
            })?;

            // Idempotency: an already-migrated memory carries `id`
            // in its frontmatter. Leave it.
            if split.frontmatter.get("id").is_some() {
                total_skipped += 1;
                continue;
            }

            let id = Uuid::now_v7();
            let mut fm = split.frontmatter.clone();
            fm.insert(
                "id".to_string(),
                toml::Value::String(id.to_string()),
            );
            let rewritten = render_memory(&fm, &split.body)?;
            let new_path = memory_path(slug, id);

            if !dry_run {
                let files = vec![
                    (new_path.clone(), Some(rewritten.into_bytes())),
                    // `(path, None)` is the delete marker consumed
                    // by `CommitSpec::build_tree`. Both edits land
                    // in the same commit so `git log` never shows
                    // the slug in two places at once.
                    (legacy_path.clone(), None),
                ];
                commit(
                    &backend,
                    &entry.handle,
                    &author,
                    format!("migrate(FR-028): move {slug} to {id}"),
                    files,
                )
                .await?;
            }

            let kind = split
                .frontmatter
                .get("kind")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            if kind == "fr" || kind == "feature" {
                fr_candidates.push((group_uuid.clone(), slug.to_string(), id));
            }
            slug_to_id.insert(slug.to_string(), id);
            total_migrated += 1;
            eprintln!("  moved {slug} -> {id}");
        }
    }

    // ── Pass 2: rewrite FR cross-refs from slugs to UUIDs ──────────
    // In dry-run mode the pass-1 moves never hit the backend, so
    // `memories/<slug>/<id>.md` doesn't exist yet. Log the planned
    // work and bail out — the real run will visit the written
    // files and do the actual rewrites.
    if dry_run {
        eprintln!(
            "[migrate_uuidify] dry-run: would re-visit {} FR memor{} for cross-ref rewrites (skipped in dry-run)",
            fr_candidates.len(),
            if fr_candidates.len() == 1 { "y" } else { "ies" }
        );
        eprintln!(
            "[migrate_uuidify] done: migrated={total_migrated} skipped={total_skipped} cross_refs_rewritten=0 (dry-run)"
        );
        return Ok(());
    }
    let mut total_cross_refs_rewritten = 0usize;
    for (group_uuid, slug, id) in &fr_candidates {
        let entry = groups
            .get(&mmcp_core::id::GroupId::from_uuid(
                Uuid::parse_str(group_uuid).expect("group uuid round-trips"),
            ))
            .await
            .ok_or_else(|| anyhow!("group {group_uuid} vanished between passes"))?;
        let new_path = memory_path(slug, *id);
        let bytes = backend
            .read_file(&entry.handle, &new_path, &Rev::head())
            .await
            .with_context(|| format!("re-reading {new_path} during cross-ref rewrite"))?;
        let text = std::str::from_utf8(&bytes)
            .with_context(|| format!("{new_path} is not valid UTF-8"))?;
        let split = split_frontmatter(text)
            .with_context(|| format!("parsing frontmatter of {new_path}"))?;

        let mut fm = split.frontmatter.clone();
        let mut rewritten_any = false;
        if let Some(feature_value) = fm.get_mut("feature") {
            if let Some(feature_table) = feature_value.as_table_mut() {
                for field in ["depends_on", "blocks"] {
                    if let Some(list) = feature_table.get_mut(field) {
                        let rewrites = rewrite_cross_ref_list(list, &slug_to_id, field, slug)?;
                        rewritten_any |= rewrites;
                    }
                }
            }
        }
        if !rewritten_any {
            continue;
        }

        let rewritten = render_memory(&fm, &split.body)?;
        if !dry_run {
            commit(
                &backend,
                &entry.handle,
                &author,
                format!("migrate(FR-028): rewrite cross-refs on {slug}"),
                vec![(new_path.clone(), Some(rewritten.into_bytes()))],
            )
            .await?;
        }
        total_cross_refs_rewritten += 1;
        eprintln!("  rewrote cross-refs on {slug}");
    }

    eprintln!(
        "[migrate_uuidify] done: migrated={total_migrated} skipped={total_skipped} cross_refs_rewritten={total_cross_refs_rewritten}"
    );
    Ok(())
}

/// Frontmatter + body split, with the frontmatter parsed as a
/// free-form TOML table so migration can mutate fields the typed
/// `MemoryFrontmatter` deserializer would reject (notably the
/// pre-FR-028 string-slug `depends_on` / `blocks` lists).
struct FrontmatterSplit {
    frontmatter: toml::value::Table,
    body: String,
}

fn split_frontmatter(text: &str) -> Result<FrontmatterSplit> {
    let stripped = text.strip_prefix('\u{feff}').unwrap_or(text);
    let after_open = stripped
        .strip_prefix("+++\n")
        .or_else(|| stripped.strip_prefix("+++\r\n"))
        .ok_or_else(|| anyhow!("memory file has no leading +++ fence"))?;
    let close_idx = after_open
        .find("\n+++\n")
        .or_else(|| after_open.find("\n+++\r\n"))
        .ok_or_else(|| anyhow!("memory file has no closing +++ fence"))?;
    let fm_str = &after_open[..close_idx];
    // Skip the closing fence plus its trailing newline.
    let after_close = &after_open[close_idx + 1..];
    let body_start = after_close
        .find('\n')
        .map(|i| i + 1)
        .unwrap_or(after_close.len());
    let body = after_close[body_start..].to_string();
    let fm: toml::value::Table =
        toml::from_str(fm_str).with_context(|| "parsing frontmatter TOML".to_string())?;
    Ok(FrontmatterSplit {
        frontmatter: fm,
        body,
    })
}

fn render_memory(frontmatter: &toml::value::Table, body: &str) -> Result<String> {
    let fm_str =
        toml::to_string(frontmatter).with_context(|| "rendering frontmatter TOML".to_string())?;
    let mut out = String::with_capacity(fm_str.len() + body.len() + 8);
    out.push_str("+++\n");
    out.push_str(&fm_str);
    if !fm_str.ends_with('\n') {
        out.push('\n');
    }
    out.push_str("+++\n");
    out.push_str(body);
    Ok(out)
}

/// Rewrite a single `depends_on` or `blocks` list in place. Each
/// entry is either a legacy slug (string, looked up in `slug_to_id`)
/// or an already-migrated UUID string (left alone). Unknown slugs
/// abort the migration with a clear error — the caller has a stale
/// or corrupted project if that happens.
fn rewrite_cross_ref_list(
    list: &mut toml::Value,
    slug_to_id: &HashMap<String, Uuid>,
    field: &'static str,
    owning_slug: &str,
) -> Result<bool> {
    let Some(entries) = list.as_array_mut() else {
        return Ok(false);
    };
    let mut any_rewrites = false;
    for entry in entries.iter_mut() {
        let raw = entry
            .as_str()
            .ok_or_else(|| anyhow!("non-string cross-ref entry in `{field}` on `{owning_slug}`"))?;
        if Uuid::parse_str(raw).is_ok() {
            continue; // already migrated
        }
        let mapped = slug_to_id.get(raw).ok_or_else(|| {
            anyhow!(
                "cross-ref `{raw}` on `{owning_slug}` has no matching memory in this group; \
                 the migration cannot rewrite it automatically"
            )
        })?;
        *entry = toml::Value::String(mapped.to_string());
        any_rewrites = true;
    }
    Ok(any_rewrites)
}

async fn commit(
    backend: &NativeBackend,
    handle: &RepoHandle,
    author: &ResolvedAuthor,
    message: String,
    files: Vec<(String, Option<Vec<u8>>)>,
) -> Result<()> {
    backend
        .write_commit(
            handle,
            CommitSpec::mmcp_commit(message, files, &author.name, &author.email),
        )
        .await
        .map(|_| ())
        .map_err(anyhow::Error::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    const LEGACY_FR: &str = "+++\nname = \"FR-001\"\ndescription = \"test\"\nkind = \"fr\"\n\n[feature]\nstatus = \"open\"\ndepends_on = [\"fr-prior\", \"fr-other\"]\nblocks = []\n+++\n## Need\n\nBody here.\n";

    #[test]
    fn split_preserves_body_and_parses_frontmatter_table() {
        let split = split_frontmatter(LEGACY_FR).expect("split");
        assert_eq!(
            split.frontmatter.get("name").and_then(|v| v.as_str()),
            Some("FR-001")
        );
        assert_eq!(
            split.frontmatter.get("kind").and_then(|v| v.as_str()),
            Some("fr")
        );
        let feature = split
            .frontmatter
            .get("feature")
            .and_then(|v| v.as_table())
            .expect("feature subtable");
        let depends = feature
            .get("depends_on")
            .and_then(|v| v.as_array())
            .expect("depends_on array");
        assert_eq!(depends.len(), 2);
        assert!(split.body.starts_with("## Need"));
    }

    #[test]
    fn render_round_trips_split_frontmatter() {
        let split = split_frontmatter(LEGACY_FR).expect("split");
        let rendered = render_memory(&split.frontmatter, &split.body).expect("render");
        // Should open and close with `+++` on its own line and
        // keep the body intact.
        assert!(rendered.starts_with("+++\n"));
        assert!(rendered.contains("\n+++\n## Need"));
        let reparsed = split_frontmatter(&rendered).expect("reparse");
        assert_eq!(reparsed.frontmatter, split.frontmatter);
        assert_eq!(reparsed.body, split.body);
    }

    #[test]
    fn rewrite_cross_ref_list_replaces_known_slugs_and_leaves_uuids() {
        let prior = Uuid::now_v7();
        let other = Uuid::now_v7();
        let mut map = HashMap::new();
        map.insert("fr-prior".to_string(), prior);
        map.insert("fr-other".to_string(), other);

        let already = Uuid::now_v7();
        let mut list = toml::Value::Array(vec![
            toml::Value::String("fr-prior".into()),
            toml::Value::String(already.to_string()),
            toml::Value::String("fr-other".into()),
        ]);
        let rewritten =
            rewrite_cross_ref_list(&mut list, &map, "depends_on", "fr-me").expect("rewrite");
        assert!(rewritten);
        let out: Vec<String> = list
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        assert_eq!(out[0], prior.to_string());
        assert_eq!(out[1], already.to_string());
        assert_eq!(out[2], other.to_string());
    }

    #[test]
    fn rewrite_cross_ref_list_errors_on_unknown_slug() {
        let mut map = HashMap::new();
        map.insert("known".to_string(), Uuid::now_v7());
        let mut list = toml::Value::Array(vec![toml::Value::String("unknown".into())]);
        let err = rewrite_cross_ref_list(&mut list, &map, "depends_on", "fr-me")
            .expect_err("must refuse unknown slug");
        let msg = err.to_string();
        assert!(
            msg.contains("unknown"),
            "error should name the missing slug: {msg}"
        );
    }
}

