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
use uuid::Uuid;

use crate::config::{find_project_root, load as load_project_config};
use crate::groups::{GroupEntry, GroupIndex};
use crate::home::ResolvedAuthor;
use crate::memory::{
    ImportError, delete_file_at_path, resolve_memory, slugify_filename, validate_slug,
    write_file_at_path, write_memory_by_id,
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

    /// A `depends_on` / `blocks` entry was not a valid UUID. Post-
    /// FR-028 cross-refs are typed as UUIDs; the surface layer
    /// (CLI + MCP) funnels every raw entry through
    /// [`parse_cross_refs`] so this error is the single source of
    /// truth for malformed cross-ref input.
    #[error("feature cross-reference '{value}' on field `{field}` is not a valid UUID")]
    InvalidCrossRef {
        field: &'static str,
        value: String,
    },
}

/// Parse a list of raw cross-reference strings (as they arrive on
/// the CLI `--depends-on` flag or the MCP `depends_on` JSON field)
/// into the `Vec<Uuid>` shape that [`AddSpec`] / [`UpdateSpec`]
/// expect. Keeps the parse / error-attribution logic in one place
/// so both surfaces report malformed input identically.
pub fn parse_cross_refs(values: &[String], field: &'static str) -> Result<Vec<Uuid>, FeatureError> {
    values
        .iter()
        .map(|raw| {
            Uuid::parse_str(raw).map_err(|_| FeatureError::InvalidCrossRef {
                field,
                value: raw.clone(),
            })
        })
        .collect()
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
    /// Explicit sequential number. Leave absent to let
    /// [`add_feature`] auto-assign `max(existing) + 1`; pin
    /// explicitly only for the FR-027 slug-migration binary that
    /// carries numbers forward from the old `fr-NNN-*` slug form.
    pub number: Option<u32>,
    pub depends_on: Vec<Uuid>,
    pub blocks: Vec<Uuid>,
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
    /// Explicit re-numbering. Callers almost never set this; it
    /// exists so the FR-027 slug-migration binary can stamp the
    /// number parsed from legacy `fr-NNN-*` slugs without racing
    /// `add_feature`'s auto-assignment.
    pub number: Option<u32>,
    pub depends_on: Option<Vec<Uuid>>,
    pub blocks: Option<Vec<Uuid>>,
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
    /// Sequential number within the group. `None` on legacy
    /// memories until the slug migration backfills them from the
    /// old `fr-NNN-*` slug prefix.
    pub number: Option<u32>,
    pub depends_on: Vec<Uuid>,
    pub blocks: Vec<Uuid>,
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

    // Auto-assign the sequential number when the caller did not
    // pin one. The FR-027 migration binary pins explicitly so
    // historic `fr-NNN-*` numbers are preserved; ordinary creates
    // pick `max(existing) + 1`. Gaps from deletes stay gaps.
    let number = match spec.number {
        Some(n) => Some(n),
        None => Some(next_feature_number(backend, entry).await?),
    };

    let id = Uuid::now_v7();
    let metadata = FeatureMetadata {
        status: spec.status,
        number,
        depends_on: spec.depends_on.clone(),
        blocks: spec.blocks.clone(),
    };
    let mut file = build_memory_file(
        spec.title.clone(),
        spec.description.clone(),
        spec.body.clone(),
        metadata,
    );
    file.frontmatter = file.frontmatter.clone().with_id(id);
    let rendered = file
        .to_string()
        .map_err(|e| FeatureError::Memory(ImportError::Render(e.to_string())))?;

    let message = spec
        .message
        .clone()
        .unwrap_or_else(|| format!("create feature {slug}"));
    let commit_id = write_memory_by_id(
        backend,
        &entry.handle,
        &slug,
        id,
        &rendered,
        author,
        false,
        Some(&message),
    )
    .await?;

    Ok(FeatureRecord {
        slug,
        title: spec.title,
        description: spec.description,
        body: spec.body,
        status: spec.status,
        number,
        depends_on: spec.depends_on,
        blocks: spec.blocks,
        commit_id,
    })
}

/// Compute the next auto-assigned feature number in the group:
/// `max(existing_numbers) + 1`, or `1` when no feature has a
/// number yet. Pre-FR-027 legacy features with no `number`
/// metadata do not contribute; the migration binary backfills
/// them from their slug prefix before new creates run.
async fn next_feature_number(
    backend: &NativeBackend,
    entry: &GroupEntry,
) -> Result<u32, FeatureError> {
    let records = list_features(backend, entry, None, true).await?;
    let max = records.iter().filter_map(|r| r.number).max();
    Ok(max.map_or(1, |n| n + 1))
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
    let resolved = resolve_memory(backend, &entry.handle, Some(slug), None)
        .await
        .map_err(FeatureError::Memory)?;
    let git_rev = match rev {
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
        .read_file(&entry.handle, &resolved.path, &git_rev)
        .await
        .map_err(|err| match err {
            mmcp_git::GitError::PathNotFound(_) => {
                FeatureError::Memory(ImportError::MemoryNotFound {
                    slug: Some(slug.to_string()),
                    id: resolved.id,
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
    let resolved = resolve_memory(backend, &entry.handle, Some(slug), None)
        .await
        .map_err(FeatureError::Memory)?;
    let current = read_feature(backend, entry, slug, None).await?;

    let title = spec.title.unwrap_or(current.title);
    let description = spec.description.unwrap_or(current.description);
    let body = spec.body.unwrap_or(current.body);
    let status = spec.status.unwrap_or(current.status);
    // `spec.number.is_some()` wins (explicit re-numbering); else
    // keep the existing value so ordinary edits don't wipe the
    // auto-assigned number.
    let number = spec.number.or(current.number);
    let depends_on = spec.depends_on.unwrap_or(current.depends_on);
    let blocks = spec.blocks.unwrap_or(current.blocks);

    let metadata = FeatureMetadata {
        status,
        number,
        depends_on: depends_on.clone(),
        blocks: blocks.clone(),
    };
    let mut file = build_memory_file(title.clone(), description.clone(), body.clone(), metadata);
    // Preserve the id pinned on disk so the rewrite hits the same
    // canonical path and stays addressable by UUID across the edit.
    if let Some(id) = resolved.id {
        file.frontmatter = file.frontmatter.clone().with_id(id);
    }
    let rendered = file
        .to_string()
        .map_err(|e| FeatureError::Memory(ImportError::Render(e.to_string())))?;

    let message = spec
        .message
        .clone()
        .unwrap_or_else(|| format!("update feature {slug}"));
    let commit_id = write_file_at_path(
        backend,
        &entry.handle,
        &resolved.path,
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
        number,
        depends_on,
        blocks,
        commit_id,
    })
}

/// Commit a deletion. Propagates `MemoryNotFound` verbatim so CLI
/// and MCP callers can distinguish "slug never existed" from "slug
/// is an unrelated memory kind" (`FeatureError::NotAFeature`).
/// Rename every feature under `old_slug` to `new_slug`, committing
/// the moves in a single atomic batch. UUIDs are stable across
/// the rename so cross-refs in other features keep resolving
/// without any further rewrite — the slug is a directory-level
/// label, not a primary key.
///
/// When multiple memories share `old_slug` (post-FR-028
/// duplicate-slug support), every entry moves in the same commit.
/// When no memory lives at `old_slug`, returns
/// [`ImportError::MemoryNotFound`] so callers don't silently
/// succeed on a non-existent rename.
pub async fn rename_feature(
    backend: &NativeBackend,
    entry: &GroupEntry,
    old_slug: &str,
    new_slug: &str,
    author: &ResolvedAuthor,
    message: Option<&str>,
) -> Result<Vec<FeatureRecord>, FeatureError> {
    validate_slug(old_slug).map_err(FeatureError::Memory)?;
    validate_slug(new_slug).map_err(FeatureError::Memory)?;
    if old_slug == new_slug {
        // Explicit short-circuit so operators don't pay a commit
        // for a no-op. A fresh listing is cheap and matches the
        // semantics callers expect from "rename to the same slug".
        return Ok(list_features_for_slug(backend, entry, old_slug).await?);
    }

    let old_dir = format!("{MEMORIES_DIR}/{old_slug}");
    let entries = backend
        .list_tree(&entry.handle, &old_dir, &Rev::head())
        .await
        .map_err(|e| FeatureError::Memory(ImportError::Git(e)))?;
    if entries.is_empty() {
        return Err(FeatureError::Memory(ImportError::MemoryNotFound {
            slug: Some(old_slug.to_string()),
            id: None,
        }));
    }

    // Plan the moves: one commit, new paths written and old paths
    // removed in the same tree rewrite so `git log` never shows a
    // half-renamed state.
    let mut moves: Vec<(String, Option<Vec<u8>>)> = Vec::with_capacity(entries.len() * 2);
    let mut moved_uuids: Vec<Uuid> = Vec::with_capacity(entries.len());
    for filename in &entries {
        let Some(stem) = filename.strip_suffix(MEMORY_EXTENSION) else {
            continue;
        };
        let Ok(id) = Uuid::parse_str(stem) else {
            // Not a UUID-named file — out-of-shape content we
            // refuse to silently move. Skip so the rename stays
            // narrow to legitimate memory files.
            continue;
        };
        let old_path = memory_path(old_slug, id);
        let new_path = memory_path(new_slug, id);
        let bytes = backend
            .read_file(&entry.handle, &old_path, &Rev::head())
            .await
            .map_err(|e| FeatureError::Memory(ImportError::Git(e)))?;

        // Reject rename when the source is not a feature; keeps
        // the tool aligned with `delete_feature`'s not-a-feature
        // guard.
        let text = String::from_utf8_lossy(&bytes).into_owned();
        let file =
            MemoryFile::parse(&text).map_err(|e| FeatureError::Memory(ImportError::Parse(e)))?;
        if file.frontmatter.kind != MemoryKind::Feature {
            return Err(FeatureError::NotAFeature {
                slug: old_slug.to_string(),
                kind: file.frontmatter.kind.as_str().to_string(),
            });
        }

        moves.push((new_path, Some(bytes.to_vec())));
        moves.push((old_path, None));
        moved_uuids.push(id);
    }
    if moved_uuids.is_empty() {
        return Err(FeatureError::Memory(ImportError::MemoryNotFound {
            slug: Some(old_slug.to_string()),
            id: None,
        }));
    }

    let fallback = format!("rename feature {old_slug} -> {new_slug}");
    let commit_message = message.unwrap_or(fallback.as_str());
    backend
        .write_commit(
            &entry.handle,
            mmcp_git::CommitSpec::mmcp_commit(
                commit_message.to_string(),
                moves,
                &author.name,
                &author.email,
            ),
        )
        .await
        .map_err(|e| FeatureError::Memory(ImportError::Git(e)))?;

    list_features_for_slug(backend, entry, new_slug).await
}

/// Read every feature currently living under `slug` in the
/// two-level layout. Shared between [`rename_feature`] and
/// future per-slug enumeration paths so the walk + record
/// construction stays in one place.
async fn list_features_for_slug(
    backend: &NativeBackend,
    entry: &GroupEntry,
    slug: &str,
) -> Result<Vec<FeatureRecord>, FeatureError> {
    let dir = format!("{MEMORIES_DIR}/{slug}");
    let filenames = backend
        .list_tree(&entry.handle, &dir, &Rev::head())
        .await
        .map_err(|e| FeatureError::Memory(ImportError::Git(e)))?;
    let mut out = Vec::with_capacity(filenames.len());
    for filename in filenames {
        let Some(stem) = filename.strip_suffix(MEMORY_EXTENSION) else {
            continue;
        };
        if Uuid::parse_str(stem).is_err() {
            continue;
        }
        let record = read_feature(backend, entry, slug, None).await?;
        out.push(record);
    }
    Ok(out)
}

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
    let resolved = resolve_memory(backend, &entry.handle, Some(slug), None)
        .await
        .map_err(FeatureError::Memory)?;
    let fallback = format!("delete feature {slug}");
    let commit_message = message.unwrap_or(fallback.as_str());
    let commit_id = delete_file_at_path(
        backend,
        &entry.handle,
        &resolved.path,
        author,
        Some(commit_message),
    )
    .await?;
    Ok(commit_id)
}

/// Enumerate FRs in the group, optionally filtered by status.
///
/// Filter precedence (FR-024):
/// 1. `status_filter = Some(x)` → include every FR whose status
///    matches, regardless of `show_all`. Explicit selector wins so
///    a caller asking for `resolved` FRs always sees them.
/// 2. `status_filter = None` + `show_all = true` → include every
///    FR. The "show me literally everything" escape hatch.
/// 3. `status_filter = None` + `show_all = false` → include only
///    `FeatureStatus::Open`. Default listing, matches the
///    "what still needs work?" mental model operators reach for.
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
    show_all: bool,
) -> Result<Vec<FeatureRecord>, FeatureError> {
    // Post-FR-028 every memory lives at `memories/<slug>/<uuid>.md`,
    // so the slug directories are the enumeration surface.
    // Pre-FR-028 flat files also still resolve cleanly through
    // the generic `read_feature` path, so fall back to listing
    // `memories/*.md` blobs for any group that has not migrated
    // yet.
    let mut slugs: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for subtree in backend
        .list_subtrees(&entry.handle, MEMORIES_DIR, &Rev::head())
        .await
        .map_err(|e| FeatureError::Memory(ImportError::Git(e)))?
    {
        slugs.insert(subtree);
    }
    for flat in backend
        .list_tree(&entry.handle, MEMORIES_DIR, &Rev::head())
        .await
        .map_err(|e| FeatureError::Memory(ImportError::Git(e)))?
    {
        if let Some(stem) = flat.strip_suffix(MEMORY_EXTENSION) {
            slugs.insert(stem.to_string());
        }
    }

    let mut out = Vec::new();
    for slug in slugs {
        match read_feature(backend, entry, &slug, None).await {
            Ok(record) => {
                let keep = match status_filter {
                    Some(want) => record.status == want,
                    None if show_all => true,
                    None => record.status == FeatureStatus::Open,
                };
                if keep {
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
    // Sort by sequential number ascending so the listing keeps a
    // natural lineage; numberless legacy entries land at the end
    // ordered by slug for a stable secondary key.
    out.sort_by(|a, b| match (a.number, b.number) {
        (Some(x), Some(y)) => x.cmp(&y),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => a.slug.cmp(&b.slug),
    });
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
        frontmatter: MemoryFrontmatter::new(title, description, MemoryKind::Feature)
            .with_feature(metadata),
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
    if file.frontmatter.kind != MemoryKind::Feature {
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
        number: metadata.number,
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

        let prior_id = Uuid::now_v7();
        let later_id = Uuid::now_v7();
        let spec = AddSpec {
            slug: Some("fr-round-trip".into()),
            title: "Round trip".into(),
            description: "Sanity test for the add/read round trip".into(),
            body: "## Need\n\nA round trip.\n".into(),
            status: FeatureStatus::Open,
            number: None,
            depends_on: vec![prior_id],
            blocks: vec![later_id],
            message: None,
        };
        let created = add_feature(scratch.backend(), &entry, spec.clone(), scratch.author())
            .await
            .expect("add");
        assert_eq!(created.slug, "fr-round-trip");
        assert_eq!(created.status, FeatureStatus::Open);
        assert_eq!(created.depends_on, vec![prior_id]);

        let loaded = read_feature(scratch.backend(), &entry, "fr-round-trip", None)
            .await
            .expect("read");
        assert_eq!(loaded.title, "Round trip");
        assert_eq!(loaded.depends_on, vec![prior_id]);
        assert_eq!(loaded.blocks, vec![later_id]);
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

    async fn seed_mixed_status_fixture(scratch: &ScratchHome) -> GroupEntry {
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
        entry
    }

    #[tokio::test]
    async fn list_filters_by_status() {
        let scratch = ScratchHome::new().await.expect("scratch home");
        let entry = seed_mixed_status_fixture(&scratch).await;

        // Explicit status selector wins over the default open-only
        // filter — even with `show_all=false` the caller receives
        // every FR matching the requested status.
        let opens = list_features(scratch.backend(), &entry, Some(FeatureStatus::Open), false)
            .await
            .expect("list open");
        let mut open_slugs: Vec<_> = opens.into_iter().map(|r| r.slug).collect();
        open_slugs.sort();
        assert_eq!(open_slugs, vec!["fr-a".to_string(), "fr-c".to_string()]);

        let resolved = list_features(
            scratch.backend(),
            &entry,
            Some(FeatureStatus::Resolved),
            false,
        )
        .await
        .expect("list resolved with show_all=false still returns matches");
        assert_eq!(
            resolved.len(),
            1,
            "explicit status filter wins over default open-only hide",
        );
    }

    #[tokio::test]
    async fn list_hides_closed_like_fr_by_default() {
        // FR-024: `list_features(None, false)` returns only FRs
        // whose status is `open`. Resolved, blocked, deferred, and
        // duplicate all drop out of the listing so the default
        // signal is "what still needs work?".
        let scratch = ScratchHome::new().await.expect("scratch home");
        let entry = seed_mixed_status_fixture(&scratch).await;

        let visible = list_features(scratch.backend(), &entry, None, false)
            .await
            .expect("default list");
        let mut slugs: Vec<_> = visible.into_iter().map(|r| r.slug).collect();
        slugs.sort();
        assert_eq!(
            slugs,
            vec!["fr-a".to_string(), "fr-c".to_string()],
            "default listing must hide resolved / blocked / duplicate FRs",
        );
    }

    #[tokio::test]
    async fn list_show_all_returns_every_status() {
        // FR-024: `show_all=true` re-includes every FR regardless
        // of status. Pairs with the hide-by-default test above.
        let scratch = ScratchHome::new().await.expect("scratch home");
        let entry = seed_mixed_status_fixture(&scratch).await;

        let all = list_features(scratch.backend(), &entry, None, true)
            .await
            .expect("list show_all");
        assert_eq!(
            all.len(),
            4,
            "show_all must re-include every FR regardless of status",
        );
    }

    #[tokio::test]
    async fn add_feature_auto_assigns_sequential_numbers_when_absent() {
        let scratch = ScratchHome::new().await.expect("scratch home");
        let seeded = scratch.seed_group("fr-group").await.expect("seed");
        let entry = scratch.groups().get(&seeded.group_id).await.expect("entry");

        let first = add_feature(
            scratch.backend(),
            &entry,
            AddSpec {
                slug: Some("first".into()),
                title: "first".into(),
                ..AddSpec::default()
            },
            scratch.author(),
        )
        .await
        .expect("first");
        let second = add_feature(
            scratch.backend(),
            &entry,
            AddSpec {
                slug: Some("second".into()),
                title: "second".into(),
                ..AddSpec::default()
            },
            scratch.author(),
        )
        .await
        .expect("second");
        let third = add_feature(
            scratch.backend(),
            &entry,
            AddSpec {
                slug: Some("third".into()),
                title: "third".into(),
                ..AddSpec::default()
            },
            scratch.author(),
        )
        .await
        .expect("third");
        assert_eq!(first.number, Some(1));
        assert_eq!(second.number, Some(2));
        assert_eq!(third.number, Some(3));
    }

    #[tokio::test]
    async fn add_feature_honors_pinned_number_and_resumes_after_gap() {
        let scratch = ScratchHome::new().await.expect("scratch home");
        let seeded = scratch.seed_group("fr-group").await.expect("seed");
        let entry = scratch.groups().get(&seeded.group_id).await.expect("entry");

        let pinned = add_feature(
            scratch.backend(),
            &entry,
            AddSpec {
                slug: Some("pin".into()),
                title: "pin".into(),
                number: Some(42),
                ..AddSpec::default()
            },
            scratch.author(),
        )
        .await
        .expect("pin");
        assert_eq!(pinned.number, Some(42));

        // Next unpinned create picks up past the highest existing
        // number. Numbers below the pin are not reused.
        let next = add_feature(
            scratch.backend(),
            &entry,
            AddSpec {
                slug: Some("next".into()),
                title: "next".into(),
                ..AddSpec::default()
            },
            scratch.author(),
        )
        .await
        .expect("next");
        assert_eq!(next.number, Some(43));
    }

    #[tokio::test]
    async fn rename_feature_moves_every_entry_under_slug() {
        let scratch = ScratchHome::new().await.expect("scratch home");
        let seeded = scratch.seed_group("fr-group").await.expect("seed");
        let entry = scratch.groups().get(&seeded.group_id).await.expect("entry");

        let original = add_feature(
            scratch.backend(),
            &entry,
            AddSpec {
                slug: Some("old-name".into()),
                title: "Original".into(),
                body: "body".into(),
                ..AddSpec::default()
            },
            scratch.author(),
        )
        .await
        .expect("seed");

        let moved = rename_feature(
            scratch.backend(),
            &entry,
            "old-name",
            "new-name",
            scratch.author(),
            None,
        )
        .await
        .expect("rename");
        assert_eq!(moved.len(), 1);
        assert_eq!(moved[0].slug, "new-name");
        assert_eq!(moved[0].number, original.number);

        // The new slug resolves; the old slug no longer does.
        read_feature(scratch.backend(), &entry, "new-name", None)
            .await
            .expect("read at new slug");
        let missing = read_feature(scratch.backend(), &entry, "old-name", None).await;
        assert!(
            matches!(
                missing,
                Err(FeatureError::Memory(ImportError::MemoryNotFound { .. }))
            ),
            "old slug must be gone after rename: got {missing:?}"
        );
    }

    #[tokio::test]
    async fn rename_feature_errors_when_old_slug_has_no_entries() {
        let scratch = ScratchHome::new().await.expect("scratch home");
        let seeded = scratch.seed_group("fr-group").await.expect("seed");
        let entry = scratch.groups().get(&seeded.group_id).await.expect("entry");

        let err = rename_feature(
            scratch.backend(),
            &entry,
            "ghost",
            "spirit",
            scratch.author(),
            None,
        )
        .await
        .expect_err("must refuse renaming a non-existent slug");
        assert!(matches!(
            err,
            FeatureError::Memory(ImportError::MemoryNotFound { slug: Some(s), .. }) if s == "ghost"
        ));
    }

    #[tokio::test]
    async fn rename_feature_same_slug_is_a_no_op_listing() {
        let scratch = ScratchHome::new().await.expect("scratch home");
        let seeded = scratch.seed_group("fr-group").await.expect("seed");
        let entry = scratch.groups().get(&seeded.group_id).await.expect("entry");

        add_feature(
            scratch.backend(),
            &entry,
            AddSpec {
                slug: Some("static".into()),
                title: "No-op".into(),
                body: "body".into(),
                ..AddSpec::default()
            },
            scratch.author(),
        )
        .await
        .expect("seed");

        let records = rename_feature(
            scratch.backend(),
            &entry,
            "static",
            "static",
            scratch.author(),
            None,
        )
        .await
        .expect("no-op rename");
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].slug, "static");
    }

    #[tokio::test]
    async fn list_features_sorts_by_number_ascending() {
        let scratch = ScratchHome::new().await.expect("scratch home");
        let seeded = scratch.seed_group("fr-group").await.expect("seed");
        let entry = scratch.groups().get(&seeded.group_id).await.expect("entry");

        // Deliberately insert out of order.
        for (slug, number) in [("third", 3), ("first", 1), ("second", 2)] {
            add_feature(
                scratch.backend(),
                &entry,
                AddSpec {
                    slug: Some(slug.into()),
                    title: slug.into(),
                    number: Some(number),
                    ..AddSpec::default()
                },
                scratch.author(),
            )
            .await
            .expect("seed");
        }

        let records = list_features(scratch.backend(), &entry, None, true)
            .await
            .expect("list");
        let slugs: Vec<_> = records.iter().map(|r| r.slug.as_str()).collect();
        assert_eq!(slugs, vec!["first", "second", "third"]);
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
