//! Typed CRUD over feature-request memories.
//!
//! Wraps the generic memory layer with FR-aware semantics: every
//! write / read through this module commits a memory whose
//! [`MemoryKind`](mmcp_core::memory::MemoryKind) is `Fr`, carrying a
//! structured [`FeatureMetadata`](mmcp_core::memory::FeatureMetadata)
//! block in frontmatter so the tool surface never has to parse the
//! body to classify a memory.
//!
//! The module is intentionally thin — it owns:
//!
//! - typed `AddSpec` / `UpdateSpec` / `FeatureRecord` shapes so the
//!   CLI, the MCP tools, and any third-party caller all converge on
//!   one input / output struct;
//! - kind enforcement (reads against a non-FR slug return
//!   `FeatureError::NotAFeature` rather than silently round-tripping
//!   a regular memory through FR helpers);
//! - list filtering by [`FeatureStatus`] so listings and diagnostics
//!   don't duplicate the status-filter predicate.
//!
//! Everything else — the git commit, the slug probe, the error
//! shapes for `GitError` — flows through `crate::memory`, keeping
//! the CRUD guarantees identical between FR and non-FR memories.

use std::path::{Path, PathBuf};

use mmcp_core::conventions::{MEMORIES_DIR, MEMORY_EXTENSION, memory_path};
use mmcp_core::id::GroupId;
use mmcp_core::memory::{
    FeatureMetadata, FeatureStatus, FrontmatterFormat, MemoryFile, MemoryFrontmatter, MemoryKind,
};
use mmcp_git::{GitBackend, NativeBackend, Rev};

use crate::config::{find_project_root, load as load_project_config};
use crate::groups::{GroupEntry, GroupIndex};
use crate::home::ResolvedAuthor;
use crate::memory::{
    ImportError, create_memory_file, delete_memory_file, slugify_filename, update_memory_file,
    validate_slug,
};

/// Errors specific to feature-request operations.
///
/// Wraps [`ImportError`] so every memory-layer failure shape
/// remains addressable, and adds the two FR-specific cases that
/// cannot arise on a generic memory: a target slug that exists but
/// is not an FR, and a missing title on create.
#[derive(Debug, thiserror::Error)]
pub enum FeatureError {
    /// Propagated from the memory CRUD primitives.
    #[error(transparent)]
    Memory(#[from] ImportError),

    /// Raised when `read_feature` / `update_feature` / `delete_feature`
    /// target a memory that exists but carries a non-Fr kind. Keeps
    /// the FR tools from silently operating on unrelated memories.
    #[error("memory '{slug}' exists in this group but is kind '{kind}', not a feature request")]
    NotAFeature { slug: String, kind: String },

    /// `add_feature` was called without a title and without a slug.
    /// The title is the only required human-readable label so either
    /// an explicit slug or a title must be supplied.
    #[error("feature title is required when no slug is provided")]
    TitleRequired,

    /// `resolve_project_group` found no `.mmcp.toml` on any ancestor
    /// of the supplied cwd. FR tools run in the project scope by
    /// default, so they refuse to operate outside an initialised
    /// project rather than silently writing into an unrelated group.
    #[error(
        "no mmcp project found: run `mmcp init project` first or `cd` into a directory with a `.mmcp.toml`"
    )]
    ProjectNotFound,

    /// `resolve_project_group` loaded the project config but could
    /// not find the backing group in the local mirror. Typically
    /// means `mmcp pull` has not yet cloned it.
    #[error(
        "project group {project_uuid} is not present in the local mirror; run `mmcp pull` or `mmcp init project` to populate it"
    )]
    ProjectGroupMissing { project_uuid: String },

    /// `.mmcp.toml` was found but failed to load. Separate variant
    /// so the wire code can disambiguate "no config" from "broken
    /// config".
    #[error("failed to load project config at {path}: {detail}")]
    ProjectConfigBroken { path: String, detail: String },
}

/// Input for [`add_feature`].
///
/// `slug` is auto-minted from the title when omitted; either the
/// slug or the title must be present. `description` is a one-line
/// summary shown in listings. `body` is the freeform markdown that
/// would have lived under `## Need` + `## Resolution` headings in
/// the old flat `fr.md` file — its structure is not parsed by this
/// layer.
#[derive(Debug, Clone, Default)]
pub struct AddSpec {
    pub slug: Option<String>,
    pub title: String,
    pub description: String,
    pub body: String,
    pub status: FeatureStatus,
    pub depends_on: Vec<String>,
    pub blocks: Vec<String>,
    /// Optional override for the git commit message; when absent,
    /// defaults to `create feature <slug>` so history stays
    /// self-describing.
    pub message: Option<String>,
}

/// Input for [`update_feature`].
///
/// Every field is optional. `Some(v)` means "replace with v";
/// `None` leaves the field untouched. For `depends_on` / `blocks`,
/// the semantics is full replacement — use `Some(Vec::new())` to
/// clear a list, `None` to leave it as-is.
#[derive(Debug, Clone, Default)]
pub struct UpdateSpec {
    pub title: Option<String>,
    pub description: Option<String>,
    pub body: Option<String>,
    pub status: Option<FeatureStatus>,
    pub depends_on: Option<Vec<String>>,
    pub blocks: Option<Vec<String>>,
    pub message: Option<String>,
}

/// Typed return shape for every read / write path.
///
/// Mirrors the on-disk frontmatter closely so downstream consumers
/// (CLI formatter, MCP JSON serializer, issue-bridge) have one
/// canonical shape to convert from.
#[derive(Debug, Clone)]
pub struct FeatureRecord {
    pub slug: String,
    pub title: String,
    pub description: String,
    pub body: String,
    pub status: FeatureStatus,
    pub depends_on: Vec<String>,
    pub blocks: Vec<String>,
    /// Commit id of the most recent write for this FR, or the head
    /// commit that produced the record on a read. Empty string on a
    /// freshly read FR whose history starts before this field was
    /// introduced — the field is a convenience for telemetry, not a
    /// correctness primitive.
    pub commit_id: String,
}

/// Create a new FR in the group. Errors with
/// `FeatureError::Memory(ImportError::MemoryAlreadyExists)` when
/// the slug already points at something on disk, mirroring the
/// strict-create contract the rest of the memory surface enforces.
pub async fn add_feature(
    backend: &NativeBackend,
    entry: &GroupEntry,
    spec: AddSpec,
    author: &ResolvedAuthor,
) -> Result<FeatureRecord, FeatureError> {
    if spec.title.trim().is_empty() && spec.slug.is_none() {
        return Err(FeatureError::TitleRequired);
    }
    let slug = match spec.slug {
        Some(raw) => raw,
        None => slugify_filename(&spec.title),
    };
    validate_slug(&slug).map_err(FeatureError::Memory)?;

    let metadata = FeatureMetadata {
        status: spec.status,
        depends_on: spec.depends_on.clone(),
        blocks: spec.blocks.clone(),
    };
    let file = build_memory_file(
        spec.title.clone(),
        spec.description.clone(),
        spec.body.clone(),
        metadata,
    );
    let rendered = file
        .to_string()
        .map_err(|e| FeatureError::Memory(ImportError::Render(e.to_string())))?;

    let message = spec
        .message
        .clone()
        .unwrap_or_else(|| format!("create feature {slug}"));
    let commit_id = create_memory_file(
        backend,
        &entry.handle,
        &slug,
        &rendered,
        author,
        Some(&message),
    )
    .await?;

    Ok(FeatureRecord {
        slug,
        title: spec.title,
        description: spec.description,
        body: spec.body,
        status: spec.status,
        depends_on: spec.depends_on,
        blocks: spec.blocks,
        commit_id,
    })
}

/// Read an FR by slug. When `rev` is `None`, reads the group's
/// current `main`; otherwise parses `rev` through
/// [`Rev`](mmcp_git::Rev) so branch names, tags, and commit hexes
/// all work the same way the generic `read_memory` tool does.
pub async fn read_feature(
    backend: &NativeBackend,
    entry: &GroupEntry,
    slug: &str,
    rev: Option<&str>,
) -> Result<FeatureRecord, FeatureError> {
    validate_slug(slug).map_err(FeatureError::Memory)?;
    let path = memory_path(slug);
    let resolved = match rev {
        // Heuristic aligned with the MCP `read_memory` tool: a
        // 40-char hex string resolves as a commit id; anything else
        // is treated as a branch or tag name. Kept in the store so
        // every consumer interprets `rev` the same way.
        Some(v) => {
            if v.len() == 40 && v.chars().all(|c| c.is_ascii_hexdigit()) {
                Rev::Commit(v.to_string())
            } else {
                Rev::Branch(v.to_string())
            }
        }
        None => Rev::head(),
    };
    let bytes = backend
        .read_file(&entry.handle, &path, &resolved)
        .await
        .map_err(|err| match err {
            mmcp_git::GitError::PathNotFound(_) => {
                FeatureError::Memory(ImportError::MemoryNotFound {
                    slug: slug.to_string(),
                })
            }
            other => FeatureError::Memory(ImportError::Git(other)),
        })?;
    let text = String::from_utf8_lossy(&bytes).into_owned();
    let file = MemoryFile::parse(&text).map_err(|e| FeatureError::Memory(ImportError::Parse(e)))?;
    record_from_file(slug, file, String::new())
}

/// Apply partial mutations and commit a new revision. At least one
/// field must be `Some`, but this module does not enforce that —
/// callers that pass an all-`None` `UpdateSpec` pay for a no-op
/// commit, which is harmless and arguably useful for retagging.
pub async fn update_feature(
    backend: &NativeBackend,
    entry: &GroupEntry,
    slug: &str,
    spec: UpdateSpec,
    author: &ResolvedAuthor,
) -> Result<FeatureRecord, FeatureError> {
    let current = read_feature(backend, entry, slug, None).await?;

    let title = spec.title.unwrap_or(current.title);
    let description = spec.description.unwrap_or(current.description);
    let body = spec.body.unwrap_or(current.body);
    let status = spec.status.unwrap_or(current.status);
    let depends_on = spec.depends_on.unwrap_or(current.depends_on);
    let blocks = spec.blocks.unwrap_or(current.blocks);

    let metadata = FeatureMetadata {
        status,
        depends_on: depends_on.clone(),
        blocks: blocks.clone(),
    };
    let file = build_memory_file(title.clone(), description.clone(), body.clone(), metadata);
    let rendered = file
        .to_string()
        .map_err(|e| FeatureError::Memory(ImportError::Render(e.to_string())))?;

    let message = spec
        .message
        .clone()
        .unwrap_or_else(|| format!("update feature {slug}"));
    let commit_id = update_memory_file(
        backend,
        &entry.handle,
        slug,
        &rendered,
        author,
        Some(&message),
    )
    .await?;

    Ok(FeatureRecord {
        slug: slug.to_string(),
        title,
        description,
        body,
        status,
        depends_on,
        blocks,
        commit_id,
    })
}

/// Commit a deletion. Propagates `MemoryNotFound` verbatim so CLI
/// and MCP callers can distinguish "slug never existed" from "slug
/// is an unrelated memory kind" (`FeatureError::NotAFeature`).
pub async fn delete_feature(
    backend: &NativeBackend,
    entry: &GroupEntry,
    slug: &str,
    author: &ResolvedAuthor,
    message: Option<&str>,
) -> Result<String, FeatureError> {
    // Guard against accidentally deleting an unrelated memory. The
    // generic delete primitive would happily drop a non-FR memory;
    // routing it through the FR tools would be a surprise.
    let existing = read_feature(backend, entry, slug, None).await;
    match existing {
        Ok(_) => {}
        Err(FeatureError::NotAFeature { .. }) => {
            return Err(FeatureError::NotAFeature {
                slug: slug.to_string(),
                kind: "other".to_string(),
            });
        }
        Err(other) => return Err(other),
    }
    let fallback = format!("delete feature {slug}");
    let commit_message = message.unwrap_or(fallback.as_str());
    let commit_id =
        delete_memory_file(backend, &entry.handle, slug, author, Some(commit_message)).await?;
    Ok(commit_id)
}

/// Enumerate every FR in the group, optionally filtered by status.
///
/// Non-FR memories in the same group are skipped silently — FRs
/// share the group with rules / snapshots / logs / references /
/// scratch notes, and listing would otherwise return a confused
/// shape. Memories whose frontmatter fails to parse are also
/// skipped; the generic `diagnose` tool is the canonical surface
/// for surfacing parse errors.
pub async fn list_features(
    backend: &NativeBackend,
    entry: &GroupEntry,
    status_filter: Option<FeatureStatus>,
) -> Result<Vec<FeatureRecord>, FeatureError> {
    let tree = backend
        .list_tree(&entry.handle, MEMORIES_DIR, &Rev::head())
        .await
        .map_err(|e| FeatureError::Memory(ImportError::Git(e)))?;

    let mut out = Vec::new();
    for entry_name in tree {
        let Some(slug) = entry_name.strip_suffix(MEMORY_EXTENSION) else {
            continue;
        };
        match read_feature(backend, entry, slug, None).await {
            Ok(record) => {
                if status_filter.is_none_or(|want| record.status == want) {
                    out.push(record);
                }
            }
            Err(FeatureError::NotAFeature { .. }) => {}
            // A parse error here propagates so listing doesn't lie
            // about missing FRs due to transient on-disk corruption.
            // The loop still continues past `NotAFeature` because
            // those are *expected* non-matches, not errors.
            Err(FeatureError::Memory(ImportError::Parse(_))) => {}
            Err(other) => return Err(other),
        }
    }
    Ok(out)
}

/// Resolve the group whose UUID is stored in the project's
/// `.mmcp.toml`, starting from `cwd` and walking ancestors the same
/// way the generic `find_project_root` does.
///
/// Errors are structured so every consumer (CLI exit code, MCP
/// tool payload, GUI banner) can branch on the precise failure mode
/// without parsing strings: no project discovered at all, config
/// unreadable, or config fine but the backing group is not in the
/// local mirror yet. The shape mirrors `bootstrap_context`'s
/// resolution logic but refuses to silently fall back to a no-op —
/// FR tools always want an error when the project context is
/// missing.
pub async fn resolve_project_group(
    groups: &GroupIndex,
    cwd: &Path,
) -> Result<(GroupEntry, PathBuf), FeatureError> {
    let root = find_project_root(cwd).ok_or(FeatureError::ProjectNotFound)?;
    let cfg = load_project_config(&root).map_err(|e| FeatureError::ProjectConfigBroken {
        path: root.join(".mmcp.toml").display().to_string(),
        detail: e.to_string(),
    })?;
    let uuid = *cfg.project_uuid.as_uuid();
    let entry = groups.get(&GroupId::from_uuid(uuid)).await.ok_or_else(|| {
        FeatureError::ProjectGroupMissing {
            project_uuid: uuid.to_string(),
        }
    })?;
    Ok((entry, root))
}

/// Build a `MemoryFile` with the Fr kind and a populated feature
/// block, ready for rendering. Shared between create and update so
/// the two paths never disagree on serialization shape.
fn build_memory_file(
    title: String,
    description: String,
    body: String,
    metadata: FeatureMetadata,
) -> MemoryFile {
    MemoryFile {
        frontmatter: MemoryFrontmatter {
            name: title,
            description,
            kind: MemoryKind::Fr,
            mandatory: false,
            version: None,
            tags: Vec::new(),
            bump_intent: None,
            feature: Some(metadata),
        },
        body,
        format: FrontmatterFormat::TomlPlus,
    }
}

/// Convert a parsed on-disk `MemoryFile` into the typed FR shape.
/// Returns `NotAFeature` when the file exists but is not kind=Fr,
/// so the FR tools never silently operate on unrelated memories.
fn record_from_file(
    slug: &str,
    file: MemoryFile,
    commit_id: String,
) -> Result<FeatureRecord, FeatureError> {
    if file.frontmatter.kind != MemoryKind::Fr {
        return Err(FeatureError::NotAFeature {
            slug: slug.to_string(),
            kind: file.frontmatter.kind.as_str().to_string(),
        });
    }
    let metadata = file.frontmatter.feature.unwrap_or_default();
    Ok(FeatureRecord {
        slug: slug.to_string(),
        title: file.frontmatter.name,
        description: file.frontmatter.description,
        body: file.body,
        status: metadata.status,
        depends_on: metadata.depends_on,
        blocks: metadata.blocks,
        commit_id,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::ScratchHome;

    #[tokio::test]
    async fn add_then_read_round_trips() {
        let scratch = ScratchHome::new().await.expect("scratch home");
        let seeded = scratch.seed_group("fr-group").await.expect("seed");
        let entry = scratch.groups().get(&seeded.group_id).await.expect("entry");

        let spec = AddSpec {
            slug: Some("fr-round-trip".into()),
            title: "Round trip".into(),
            description: "Sanity test for the add/read round trip".into(),
            body: "## Need\n\nA round trip.\n".into(),
            status: FeatureStatus::Open,
            depends_on: vec!["fr-prior".into()],
            blocks: vec!["fr-later".into()],
            message: None,
        };
        let created = add_feature(scratch.backend(), &entry, spec.clone(), scratch.author())
            .await
            .expect("add");
        assert_eq!(created.slug, "fr-round-trip");
        assert_eq!(created.status, FeatureStatus::Open);
        assert_eq!(created.depends_on, vec!["fr-prior".to_string()]);

        let loaded = read_feature(scratch.backend(), &entry, "fr-round-trip", None)
            .await
            .expect("read");
        assert_eq!(loaded.title, "Round trip");
        assert_eq!(loaded.depends_on, vec!["fr-prior".to_string()]);
        assert_eq!(loaded.blocks, vec!["fr-later".to_string()]);
        // Parser trims trailing newlines; compare structurally.
        assert_eq!(loaded.body.trim_end(), "## Need\n\nA round trip.");
    }

    #[tokio::test]
    async fn add_auto_mints_slug_from_title_when_absent() {
        let scratch = ScratchHome::new().await.expect("scratch home");
        let seeded = scratch.seed_group("fr-group").await.expect("seed");
        let entry = scratch.groups().get(&seeded.group_id).await.expect("entry");

        let spec = AddSpec {
            slug: None,
            title: "Auto-minted slug".into(),
            description: "Slug derived from title".into(),
            body: "body".into(),
            ..AddSpec::default()
        };
        let record = add_feature(scratch.backend(), &entry, spec, scratch.author())
            .await
            .expect("add");
        // slugify lowercases and hyphenates.
        assert_eq!(record.slug, "auto-minted-slug");
    }

    #[tokio::test]
    async fn update_replaces_status_and_preserves_other_fields() {
        let scratch = ScratchHome::new().await.expect("scratch home");
        let seeded = scratch.seed_group("fr-group").await.expect("seed");
        let entry = scratch.groups().get(&seeded.group_id).await.expect("entry");

        add_feature(
            scratch.backend(),
            &entry,
            AddSpec {
                slug: Some("fr-upd".into()),
                title: "Before".into(),
                description: "unchanged".into(),
                body: "unchanged body".into(),
                ..AddSpec::default()
            },
            scratch.author(),
        )
        .await
        .expect("seed feature");

        let updated = update_feature(
            scratch.backend(),
            &entry,
            "fr-upd",
            UpdateSpec {
                status: Some(FeatureStatus::Resolved),
                ..UpdateSpec::default()
            },
            scratch.author(),
        )
        .await
        .expect("update");

        assert_eq!(updated.status, FeatureStatus::Resolved);
        assert_eq!(updated.title, "Before", "title preserved across update");
        assert_eq!(
            updated.description, "unchanged",
            "description preserved across update"
        );
        assert_eq!(
            updated.body, "unchanged body",
            "body preserved across update"
        );
    }

    #[tokio::test]
    async fn list_filters_by_status() {
        let scratch = ScratchHome::new().await.expect("scratch home");
        let seeded = scratch.seed_group("fr-group").await.expect("seed");
        let entry = scratch.groups().get(&seeded.group_id).await.expect("entry");

        for (slug, status) in [
            ("fr-a", FeatureStatus::Open),
            ("fr-b", FeatureStatus::Resolved),
            ("fr-c", FeatureStatus::Open),
            ("fr-d", FeatureStatus::Blocked),
        ] {
            add_feature(
                scratch.backend(),
                &entry,
                AddSpec {
                    slug: Some(slug.into()),
                    title: slug.into(),
                    description: "t".into(),
                    body: "b".into(),
                    status,
                    ..AddSpec::default()
                },
                scratch.author(),
            )
            .await
            .expect("seed");
        }

        let opens = list_features(scratch.backend(), &entry, Some(FeatureStatus::Open))
            .await
            .expect("list open");
        let mut open_slugs: Vec<_> = opens.into_iter().map(|r| r.slug).collect();
        open_slugs.sort();
        assert_eq!(open_slugs, vec!["fr-a".to_string(), "fr-c".to_string()]);

        let all = list_features(scratch.backend(), &entry, None)
            .await
            .expect("list all");
        assert_eq!(all.len(), 4, "list without filter returns every FR");
    }

    #[tokio::test]
    async fn read_on_non_fr_memory_errors_with_not_a_feature() {
        use crate::memory::{SynthFrontmatter, import_memory};
        let scratch = ScratchHome::new().await.expect("scratch home");
        let seeded = scratch.seed_group("fr-group").await.expect("seed");
        let entry = scratch.groups().get(&seeded.group_id).await.expect("entry");

        // Seed a regular rule memory via the generic memory layer.
        import_memory(
            scratch.backend(),
            &entry.handle,
            "not-a-feature",
            "rule body",
            Some(SynthFrontmatter {
                name: "Plain rule".into(),
                description: "not an FR".into(),
                kind: MemoryKind::Rule,
            }),
            scratch.author(),
            false,
        )
        .await
        .expect("seed non-FR memory");

        let err = read_feature(scratch.backend(), &entry, "not-a-feature", None)
            .await
            .expect_err("read non-FR must fail with NotAFeature");
        match err {
            FeatureError::NotAFeature { slug, kind } => {
                assert_eq!(slug, "not-a-feature");
                assert_eq!(kind, "rule");
            }
            other => panic!("expected NotAFeature, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn delete_refuses_when_slug_is_not_an_fr() {
        use crate::memory::{SynthFrontmatter, import_memory};
        let scratch = ScratchHome::new().await.expect("scratch home");
        let seeded = scratch.seed_group("fr-group").await.expect("seed");
        let entry = scratch.groups().get(&seeded.group_id).await.expect("entry");

        import_memory(
            scratch.backend(),
            &entry.handle,
            "guarded-rule",
            "rule body",
            Some(SynthFrontmatter {
                name: "Plain rule".into(),
                description: "not an FR".into(),
                kind: MemoryKind::Rule,
            }),
            scratch.author(),
            false,
        )
        .await
        .expect("seed non-FR memory");

        let err = delete_feature(
            scratch.backend(),
            &entry,
            "guarded-rule",
            scratch.author(),
            None,
        )
        .await
        .expect_err("delete on a non-FR must refuse");
        assert!(matches!(err, FeatureError::NotAFeature { .. }));
    }
}
