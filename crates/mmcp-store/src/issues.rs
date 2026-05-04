//! Typed CRUD over issue tracker memories.
//!
//! Sister surface to [`crate::features`]. Wraps the generic memory
//! layer with issue-aware semantics: every write / read commits a
//! memory whose [`MemoryKind`] is `Issue`, carrying a structured
//! [`IssueMetadata`] block in frontmatter. Cross-references take
//! any UUID, so an issue may depend on a feature, another issue,
//! or any future tracker kind. The shared ticket counter lives in
//! [`crate::tracker`] so feature and issue numbers occupy one
//! per-group monotonic sequence (GitHub-style).
//!
//! The hybrid model from the design discussion permits a memory
//! to carry both a `[feature]` and an `[issue]` block. This
//! module's create path always writes a pure-issue memory; hybrid
//! promotion is a future workflow on top of the existing memory
//! editor surface.
//!
//! Cross-kind and cross-group supersede generalisation is staged
//! for a later slice. v1 supersede targets resolve in the caller's
//! project group only; the data model itself supports the broader
//! shape.

use mmcp_core::memory::{
    FrontmatterFormat, IssueMetadata, IssueStatus, MemoryFile, MemoryFrontmatter, MemoryKind,
    MemoryRef,
};
use mmcp_git::{GitBackend, NativeBackend, Rev};
use uuid::Uuid;

use crate::groups::GroupEntry;
use crate::home::ResolvedAuthor;
use crate::memory::{
    AddressingMode, ImportError, delete_file_at_path, resolve_memory, slugify_filename,
    validate_slug, write_file_at_path, write_memory_by_id,
};

/// Errors specific to issue-tracker operations.
#[derive(Debug, thiserror::Error)]
pub enum IssueError {
    /// Propagated from the memory CRUD primitives.
    #[error(transparent)]
    Memory(#[from] ImportError),

    /// Raised when `read_issue` / `update_issue` / `delete_issue`
    /// target a memory that exists but is neither an `Issue` kind
    /// nor a hybrid memory carrying an `[issue]` block. Keeps the
    /// issue tools from silently operating on unrelated memories.
    #[error("memory '{slug}' exists in this group but does not carry an issue metadata block")]
    NotAnIssue { slug: String, kind: String },

    /// `add_issue` was called without a title and without a slug.
    #[error("issue title is required when no slug is provided")]
    TitleRequired,

    /// Malformed cross-reference parsed through `mmcp_core::memory::xrefs`.
    #[error(transparent)]
    Xref(#[from] mmcp_core::memory::XrefError),

    /// `supersedes` pointed at a slug / UUID the local mirror could
    /// not resolve in the caller's project group.
    #[error("supersedes target '{query}' does not resolve in this project group")]
    SupersedesUnknown { query: String },

    /// `supersedes` pointed at an issue whose current status rules
    /// out supersession.
    #[error("supersedes target '{slug}' has status '{}' which cannot be superseded", status.as_str())]
    SupersedesInvalidStatus {
        slug: String,
        status: IssueStatus,
        existing_link: Option<MemoryRef>,
    },
}

/// Input for [`add_issue`].
#[derive(Debug, Clone, Default)]
pub struct AddSpec {
    pub slug: Option<String>,
    pub title: String,
    pub description: String,
    pub body: String,
    pub status: IssueStatus,
    /// Optional explicit number override. Leave absent to let
    /// `add_issue` mint the next sequential number from the shared
    /// tracker counter.
    pub number: Option<u32>,
    pub depends_on: Vec<Uuid>,
    pub blocks: Vec<Uuid>,
    pub refs: Vec<MemoryRef>,
    /// Slug or UUID of an existing issue in the same project group
    /// to supersede. Cross-kind targets (a feature) are not
    /// resolved here in v1; that capability is staged for a later
    /// slice.
    pub supersedes: Option<String>,
    /// FR-38 provenance UUID.
    pub source: Option<Uuid>,
    pub message: Option<String>,
}

/// Input for [`update_issue`].
#[derive(Debug, Clone, Default)]
pub struct UpdateSpec {
    pub title: Option<String>,
    pub description: Option<String>,
    pub body: Option<String>,
    pub status: Option<IssueStatus>,
    pub depends_on: Option<Vec<Uuid>>,
    pub blocks: Option<Vec<Uuid>>,
    pub refs_add: Option<Vec<MemoryRef>>,
    pub refs_remove: Option<Vec<Uuid>>,
    pub superseded_by: Option<MemoryRef>,
    pub message: Option<String>,
}

/// Typed return shape for every read / write path on the issue
/// surface. Mirrors the on-disk frontmatter closely so downstream
/// consumers (CLI formatter, MCP JSON serializer, future
/// issue-bridge) have one canonical shape to convert from.
#[derive(Debug, Clone)]
pub struct IssueRecord {
    pub slug: String,
    pub title: String,
    pub description: String,
    pub body: String,
    pub status: IssueStatus,
    pub number: Option<u32>,
    pub depends_on: Vec<Uuid>,
    pub blocks: Vec<Uuid>,
    pub superseded_by: Option<MemoryRef>,
    pub commit_id: String,
}

/// Body-free projection of an [`IssueRecord`] for list-style surfaces.
#[derive(Debug, Clone)]
pub struct IssueSummary {
    pub slug: String,
    pub title: String,
    pub description: String,
    pub status: IssueStatus,
    pub number: Option<u32>,
    pub depends_on: Vec<Uuid>,
    pub blocks: Vec<Uuid>,
    pub superseded_by: Option<MemoryRef>,
    pub commit_id: String,
}

impl IssueSummary {
    fn from_record(record: IssueRecord) -> Self {
        let IssueRecord {
            slug,
            title,
            description,
            body: _,
            status,
            number,
            depends_on,
            blocks,
            superseded_by,
            commit_id,
        } = record;
        Self {
            slug,
            title,
            description,
            status,
            number,
            depends_on,
            blocks,
            superseded_by,
            commit_id,
        }
    }
}

/// Internal resolved form of an `AddSpec::supersedes` target.
struct SupersedeTarget {
    slug: String,
    id: Uuid,
    head_commit: String,
}

/// Create a new issue in the group. Errors with
/// `IssueError::Memory(ImportError::MemoryAlreadyExists)` when the
/// slug already points at something on disk.
///
/// When `spec.supersedes` is set, runs the two-commit supersede
/// flow against the resolved target (same kind, same group only
/// in v1).
pub async fn add_issue(
    backend: &NativeBackend,
    entry: &GroupEntry,
    spec: AddSpec,
    author: &ResolvedAuthor,
) -> Result<IssueRecord, IssueError> {
    let group = *entry.manifest.group_id.as_uuid();
    let _guards =
        crate::lock::acquire_chain(&crate::lock::create_chain(group)).await;

    if spec.title.trim().is_empty() && spec.slug.is_none() {
        return Err(IssueError::TitleRequired);
    }
    let slug = match spec.slug.clone() {
        Some(raw) => raw,
        None => slugify_filename(&spec.title),
    };
    validate_slug(&slug).map_err(IssueError::Memory)?;

    let supersede_target = match spec.supersedes.as_deref() {
        Some(query) => Some(resolve_supersede_target(backend, entry, query).await?),
        None => None,
    };

    let number = match spec.number {
        Some(n) => Some(n),
        None => Some(
            crate::tracker::next_ticket_number(backend, entry)
                .await
                .map_err(IssueError::Memory)?,
        ),
    };

    let id = Uuid::now_v7();

    let mut refs = spec.refs.clone();
    if let Some(target) = supersede_target.as_ref() {
        let auto_ref = MemoryRef::new(target.id, target.head_commit.clone());
        if !refs.iter().any(|r| r.target == target.id) {
            refs.push(auto_ref);
        }
    }

    let metadata = IssueMetadata {
        status: spec.status,
        number,
        depends_on: spec.depends_on.clone(),
        blocks: spec.blocks.clone(),
        superseded_by: None,
    };
    let mut file = build_memory_file(
        spec.title.clone(),
        spec.description.clone(),
        spec.body.clone(),
        metadata,
    );
    file.frontmatter = file
        .frontmatter
        .clone()
        .with_id(id)
        .with_refs(refs.clone())
        .with_source(spec.source);
    let rendered = file
        .to_string()
        .map_err(|e| IssueError::Memory(ImportError::Render(e.to_string())))?;

    let message = spec
        .message
        .clone()
        .unwrap_or_else(|| match supersede_target.as_ref() {
            Some(t) => format!("create issue {slug} (supersedes {})", t.slug),
            None => format!("create issue {slug}"),
        });
    let (commit_id, _validation) = write_memory_by_id(
        backend,
        &entry.handle,
        &slug,
        id,
        &rendered,
        author,
        false,
        AddressingMode::BySlugOnly,
        false,
        Some(&message),
    )
    .await?;

    if let Some(target) = supersede_target {
        let back_link = MemoryRef::new(id, commit_id.clone());
        let retry_message = format!("mark {} superseded by {slug}", target.slug);
        update_issue_unlocked(
            backend,
            entry,
            &target.slug,
            UpdateSpec {
                status: Some(IssueStatus::Superseded),
                superseded_by: Some(back_link),
                message: Some(retry_message),
                ..UpdateSpec::default()
            },
            author,
        )
        .await?;
    }

    Ok(IssueRecord {
        slug,
        title: spec.title,
        description: spec.description,
        body: spec.body,
        status: spec.status,
        number,
        depends_on: spec.depends_on,
        blocks: spec.blocks,
        superseded_by: None,
        commit_id,
    })
}

async fn resolve_supersede_target(
    backend: &NativeBackend,
    entry: &GroupEntry,
    query: &str,
) -> Result<SupersedeTarget, IssueError> {
    let record = match read_issue(backend, entry, query, None).await {
        Ok(r) => r,
        Err(IssueError::Memory(ImportError::MemoryNotFound { .. })) => {
            return Err(IssueError::SupersedesUnknown {
                query: query.to_string(),
            });
        }
        Err(other) => return Err(other),
    };

    match record.status {
        IssueStatus::Open
        | IssueStatus::Blocked
        | IssueStatus::Deferred
        | IssueStatus::Closed => {}
        IssueStatus::Superseded => {
            return Err(IssueError::SupersedesInvalidStatus {
                slug: record.slug,
                status: IssueStatus::Superseded,
                existing_link: record.superseded_by,
            });
        }
        other => {
            return Err(IssueError::SupersedesInvalidStatus {
                slug: record.slug,
                status: other,
                existing_link: None,
            });
        }
    }

    let resolved = resolve_memory(backend, &entry.handle, Some(&record.slug), None)
        .await
        .map_err(IssueError::Memory)?;

    Ok(SupersedeTarget {
        slug: record.slug,
        id: resolved.id,
        head_commit: record.commit_id,
    })
}

/// Read an issue by slug.
pub async fn read_issue(
    backend: &NativeBackend,
    entry: &GroupEntry,
    slug: &str,
    rev: Option<&str>,
) -> Result<IssueRecord, IssueError> {
    validate_slug(slug).map_err(IssueError::Memory)?;
    let resolved = resolve_memory(backend, &entry.handle, Some(slug), None)
        .await
        .map_err(IssueError::Memory)?;
    let git_rev = match rev {
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
                IssueError::Memory(ImportError::MemoryNotFound {
                    slug: Some(slug.to_string()),
                    id: Some(resolved.id),
                })
            }
            other => IssueError::Memory(ImportError::Git(other)),
        })?;
    let text = String::from_utf8_lossy(&bytes).into_owned();
    let file = MemoryFile::parse(&text).map_err(|e| IssueError::Memory(ImportError::Parse(e)))?;
    record_from_file(slug, file, String::new())
}

/// Update an issue by slug. Public wrapper acquiring the per-group
/// lock chain; delegates to [`update_issue_unlocked`].
pub async fn update_issue(
    backend: &NativeBackend,
    entry: &GroupEntry,
    slug: &str,
    spec: UpdateSpec,
    author: &ResolvedAuthor,
) -> Result<IssueRecord, IssueError> {
    let group = *entry.manifest.group_id.as_uuid();
    let _ancestors = crate::lock::acquire_chain(&[
        (crate::lock::LockScope::Process, crate::lock::LockMode::Shared),
        (crate::lock::LockScope::Group(group), crate::lock::LockMode::Shared),
    ])
    .await;
    let resolved = resolve_memory(backend, &entry.handle, Some(slug), None)
        .await
        .map_err(IssueError::Memory)?;
    let _leaf = crate::lock::acquire(
        crate::lock::LockScope::Memory(resolved.id),
        crate::lock::LockMode::Exclusive,
    )
    .await;
    update_issue_unlocked(backend, entry, slug, spec, author).await
}

/// Inner non-locking variant for use by the supersede flow's
/// commit B (which already holds the per-group lock).
pub async fn update_issue_unlocked(
    backend: &NativeBackend,
    entry: &GroupEntry,
    slug: &str,
    spec: UpdateSpec,
    author: &ResolvedAuthor,
) -> Result<IssueRecord, IssueError> {
    let resolved = resolve_memory(backend, &entry.handle, Some(slug), None)
        .await
        .map_err(IssueError::Memory)?;
    let current = read_issue(backend, entry, slug, None).await?;
    let current_refs = read_memory_refs(backend, &entry.handle, &resolved.path).await?;

    let title = spec.title.unwrap_or(current.title);
    let description = spec.description.unwrap_or(current.description);
    let body = spec.body.unwrap_or(current.body);
    let status = spec.status.unwrap_or(current.status);
    // Numbers are immutable after create; preserve the on-disk value.
    let number = current.number;
    let depends_on = spec.depends_on.unwrap_or(current.depends_on);
    let blocks = spec.blocks.unwrap_or(current.blocks);
    let refs = compose_refs(current_refs, spec.refs_remove.as_deref(), spec.refs_add.as_deref());
    let superseded_by = spec.superseded_by.or(current.superseded_by);

    let metadata = IssueMetadata {
        status,
        number,
        depends_on: depends_on.clone(),
        blocks: blocks.clone(),
        superseded_by: superseded_by.clone(),
    };
    metadata
        .validate_supersede_invariant()
        .map_err(|e| IssueError::Memory(ImportError::Render(e.to_string())))?;

    let mut file = build_memory_file(title.clone(), description.clone(), body.clone(), metadata);
    file.frontmatter = file
        .frontmatter
        .clone()
        .with_id(resolved.id)
        .with_refs(refs);
    let rendered = file
        .to_string()
        .map_err(|e| IssueError::Memory(ImportError::Render(e.to_string())))?;

    let message = spec
        .message
        .clone()
        .unwrap_or_else(|| format!("update issue {slug}"));
    let (commit_id, _validation) = write_file_at_path(
        backend,
        &entry.handle,
        &resolved.path,
        &rendered,
        author,
        resolved.addressing_mode,
        false,
        Some(&message),
    )
    .await?;

    Ok(IssueRecord {
        slug: slug.to_string(),
        title,
        description,
        body,
        status,
        number,
        depends_on,
        blocks,
        superseded_by,
        commit_id,
    })
}

async fn read_memory_refs(
    backend: &NativeBackend,
    handle: &mmcp_git::RepoHandle,
    path: &str,
) -> Result<Vec<MemoryRef>, IssueError> {
    let bytes = backend
        .read_file(handle, path, &Rev::head())
        .await
        .map_err(|e| IssueError::Memory(ImportError::Git(e)))?;
    let text = std::str::from_utf8(&bytes)
        .map_err(|e| IssueError::Memory(ImportError::Render(e.to_string())))?;
    let file = MemoryFile::parse(text).map_err(|e| IssueError::Memory(ImportError::Parse(e)))?;
    Ok(file.frontmatter.refs)
}

fn compose_refs(
    current: Vec<MemoryRef>,
    remove: Option<&[Uuid]>,
    add: Option<&[MemoryRef]>,
) -> Vec<MemoryRef> {
    let mut out = current;
    if let Some(remove) = remove {
        out.retain(|r| !remove.contains(&r.target));
    }
    if let Some(add) = add {
        for new in add {
            out.retain(|r| r.target != new.target);
            out.push(new.clone());
        }
    }
    out
}

/// Rename every issue under `old_slug` to `new_slug` in one atomic
/// commit. UUIDs stay stable across the rename.
pub async fn rename_issue(
    backend: &NativeBackend,
    entry: &GroupEntry,
    old_slug: &str,
    new_slug: &str,
    author: &ResolvedAuthor,
    message: Option<&str>,
) -> Result<Vec<IssueRecord>, IssueError> {
    let _guards = crate::lock::acquire_chain(&crate::lock::coarsen_group_chain(
        *entry.manifest.group_id.as_uuid(),
    ))
    .await;
    validate_slug(old_slug).map_err(IssueError::Memory)?;
    validate_slug(new_slug).map_err(IssueError::Memory)?;
    if old_slug == new_slug {
        return list_issues_for_slug(backend, entry, old_slug).await;
    }

    let old_dir = format!(
        "{}/{old_slug}",
        mmcp_core::conventions::MEMORIES_DIR
    );
    let entries = backend
        .list_tree(&entry.handle, &old_dir, &Rev::head())
        .await
        .map_err(|e| IssueError::Memory(ImportError::Git(e)))?;
    if entries.is_empty() {
        return Err(IssueError::Memory(ImportError::MemoryNotFound {
            slug: Some(old_slug.to_string()),
            id: None,
        }));
    }

    let mut moves: Vec<(String, Option<Vec<u8>>)> = Vec::with_capacity(entries.len() * 2);
    let mut moved = 0usize;
    for filename in &entries {
        let Some(stem) = filename.strip_suffix(mmcp_core::conventions::MEMORY_EXTENSION) else {
            continue;
        };
        let Ok(id) = Uuid::parse_str(stem) else {
            continue;
        };
        let old_path = mmcp_core::conventions::memory_path(old_slug, id);
        let new_path = mmcp_core::conventions::memory_path(new_slug, id);
        let bytes = backend
            .read_file(&entry.handle, &old_path, &Rev::head())
            .await
            .map_err(|e| IssueError::Memory(ImportError::Git(e)))?;

        let text = String::from_utf8_lossy(&bytes).into_owned();
        let file = MemoryFile::parse(&text)
            .map_err(|e| IssueError::Memory(ImportError::Parse(e)))?;
        // Refuse rename when the source memory does not carry an
        // [issue] block. Mirrors the feature side's
        // not-a-feature guard.
        if file.frontmatter.issue.is_none() {
            return Err(IssueError::NotAnIssue {
                slug: old_slug.to_string(),
                kind: file.frontmatter.kind.as_str().to_string(),
            });
        }

        moves.push((new_path, Some(bytes.to_vec())));
        moves.push((old_path, None));
        moved += 1;
    }
    if moved == 0 {
        return Err(IssueError::Memory(ImportError::MemoryNotFound {
            slug: Some(old_slug.to_string()),
            id: None,
        }));
    }

    let fallback = format!("rename issue {old_slug} -> {new_slug}");
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
        .map_err(|e| IssueError::Memory(ImportError::Git(e)))?;

    list_issues_for_slug(backend, entry, new_slug).await
}

async fn list_issues_for_slug(
    backend: &NativeBackend,
    entry: &GroupEntry,
    slug: &str,
) -> Result<Vec<IssueRecord>, IssueError> {
    let dir = format!("{}/{slug}", mmcp_core::conventions::MEMORIES_DIR);
    let filenames = backend
        .list_tree(&entry.handle, &dir, &Rev::head())
        .await
        .map_err(|e| IssueError::Memory(ImportError::Git(e)))?;
    let mut out = Vec::with_capacity(filenames.len());
    for filename in filenames {
        let Some(stem) = filename.strip_suffix(mmcp_core::conventions::MEMORY_EXTENSION) else {
            continue;
        };
        if Uuid::parse_str(stem).is_err() {
            continue;
        }
        let record = read_issue(backend, entry, slug, None).await?;
        out.push(record);
    }
    Ok(out)
}

/// Delete an issue by slug.
pub async fn delete_issue(
    backend: &NativeBackend,
    entry: &GroupEntry,
    slug: &str,
    author: &ResolvedAuthor,
    message: Option<&str>,
) -> Result<String, IssueError> {
    let group = *entry.manifest.group_id.as_uuid();
    let _ancestors = crate::lock::acquire_chain(&[
        (crate::lock::LockScope::Process, crate::lock::LockMode::Shared),
        (crate::lock::LockScope::Group(group), crate::lock::LockMode::Shared),
    ])
    .await;
    let resolved = resolve_memory(backend, &entry.handle, Some(slug), None)
        .await
        .map_err(IssueError::Memory)?;
    let _leaf = crate::lock::acquire(
        crate::lock::LockScope::Memory(resolved.id),
        crate::lock::LockMode::Exclusive,
    )
    .await;
    let existing = read_issue(backend, entry, slug, None).await;
    match existing {
        Ok(_) => {}
        Err(IssueError::NotAnIssue { .. }) => {
            return Err(IssueError::NotAnIssue {
                slug: slug.to_string(),
                kind: "other".to_string(),
            });
        }
        Err(other) => return Err(other),
    }
    let resolved = resolve_memory(backend, &entry.handle, Some(slug), None)
        .await
        .map_err(IssueError::Memory)?;
    let fallback = format!("delete issue {slug}");
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

/// Enumerate issues in the group, optionally filtered by status.
///
/// Mirrors `list_features` semantics:
/// - `status_filter = Some(x)` always wins over `show_all`.
/// - `show_all = true` returns every status.
/// - default (`None` + `false`) hides terminal-ish states
///   (Closed, Wontfix, Duplicate, Superseded).
///
/// Per Q13, this returns every memory in the group whose
/// frontmatter carries an `[issue]` block, regardless of the
/// `kind` discriminator. Hybrid memories appear in both
/// `list_features` and `list_issues`.
pub async fn list_issues(
    backend: &NativeBackend,
    entry: &GroupEntry,
    status_filter: Option<IssueStatus>,
    show_all: bool,
) -> Result<Vec<IssueRecord>, IssueError> {
    // FR-41-aware: walk recursively so nested slug paths surface
    // alongside flat ones.
    let slug_dirs = crate::memory::list_memory_slug_dirs(backend, &entry.handle, &Rev::head())
        .await
        .map_err(|e| IssueError::Memory(ImportError::Git(e)))?;

    let mut out = Vec::new();
    for slug_dir in slug_dirs {
        match read_issue(backend, entry, &slug_dir.slug, None).await {
            Ok(record) => {
                let keep = match status_filter {
                    Some(want) => record.status == want,
                    None if show_all => true,
                    None => !record.status.is_default_hidden(),
                };
                if keep {
                    out.push(record);
                }
            }
            Err(IssueError::NotAnIssue { .. }) => {}
            Err(IssueError::Memory(ImportError::Parse(_))) => {}
            Err(other) => return Err(other),
        }
    }
    out.sort_by(|a, b| match (a.number, b.number) {
        (Some(x), Some(y)) => x.cmp(&y),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => a.slug.cmp(&b.slug),
    });
    Ok(out)
}

/// Body-free counterpart to [`list_issues`] for listing surfaces.
pub async fn list_issue_summaries(
    backend: &NativeBackend,
    entry: &GroupEntry,
    status_filter: Option<IssueStatus>,
    show_all: bool,
) -> Result<Vec<IssueSummary>, IssueError> {
    let records = list_issues(backend, entry, status_filter, show_all).await?;
    Ok(records.into_iter().map(IssueSummary::from_record).collect())
}

fn build_memory_file(
    title: String,
    description: String,
    body: String,
    metadata: IssueMetadata,
) -> MemoryFile {
    MemoryFile {
        frontmatter: MemoryFrontmatter::new(title, description, MemoryKind::Issue)
            .with_issue(metadata),
        body,
        format: FrontmatterFormat::TomlPlus,
    }
}

fn record_from_file(
    slug: &str,
    file: MemoryFile,
    commit_id: String,
) -> Result<IssueRecord, IssueError> {
    let metadata = match file.frontmatter.issue {
        Some(meta) => meta,
        None => {
            return Err(IssueError::NotAnIssue {
                slug: slug.to_string(),
                kind: file.frontmatter.kind.as_str().to_string(),
            });
        }
    };
    Ok(IssueRecord {
        slug: slug.to_string(),
        title: file.frontmatter.name,
        description: file.frontmatter.description,
        body: file.body,
        status: metadata.status,
        number: metadata.number,
        depends_on: metadata.depends_on,
        blocks: metadata.blocks,
        superseded_by: metadata.superseded_by,
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
        let seeded = scratch.seed_group("issue-group").await.expect("seed");
        let entry = scratch.groups().get(&seeded.group_id).await.expect("entry");

        let spec = AddSpec {
            slug: Some("first-issue".into()),
            title: "First issue".into(),
            description: "round trip test".into(),
            body: "## Repro\n\nfoo\n".into(),
            status: IssueStatus::Open,
            ..AddSpec::default()
        };
        let created = add_issue(scratch.backend(), &entry, spec, scratch.author())
            .await
            .expect("add");
        assert_eq!(created.slug, "first-issue");
        assert_eq!(created.status, IssueStatus::Open);
        assert_eq!(created.number, Some(1));

        let loaded = read_issue(scratch.backend(), &entry, "first-issue", None)
            .await
            .expect("read");
        assert_eq!(loaded.title, "First issue");
        assert_eq!(loaded.body.trim_end(), "## Repro\n\nfoo");
    }

    #[tokio::test]
    async fn auto_mints_slug_from_title_when_absent() {
        let scratch = ScratchHome::new().await.expect("scratch home");
        let seeded = scratch.seed_group("issue-group").await.expect("seed");
        let entry = scratch.groups().get(&seeded.group_id).await.expect("entry");

        let spec = AddSpec {
            slug: None,
            title: "Something Broken".into(),
            description: "auto slug".into(),
            body: "body".into(),
            ..AddSpec::default()
        };
        let record = add_issue(scratch.backend(), &entry, spec, scratch.author())
            .await
            .expect("add");
        assert_eq!(record.slug, "something-broken");
    }

    #[tokio::test]
    async fn shared_counter_skips_existing_feature_numbers() {
        let scratch = ScratchHome::new().await.expect("scratch home");
        let seeded = scratch.seed_group("mixed-group").await.expect("seed");
        let entry = scratch.groups().get(&seeded.group_id).await.expect("entry");

        crate::features::add_feature(
            scratch.backend(),
            &entry,
            crate::features::AddSpec {
                slug: Some("a-feat".into()),
                title: "feat one".into(),
                description: "shared counter test".into(),
                body: "x".into(),
                ..crate::features::AddSpec::default()
            },
            scratch.author(),
        )
        .await
        .expect("seed feature");

        let issue = add_issue(
            scratch.backend(),
            &entry,
            AddSpec {
                slug: Some("an-issue".into()),
                title: "issue one".into(),
                description: "shared counter test".into(),
                body: "y".into(),
                ..AddSpec::default()
            },
            scratch.author(),
        )
        .await
        .expect("add issue");
        assert_eq!(
            issue.number,
            Some(2),
            "issue must skip the existing feature number"
        );
    }

    #[tokio::test]
    async fn update_replaces_status_and_preserves_other_fields() {
        let scratch = ScratchHome::new().await.expect("scratch home");
        let seeded = scratch.seed_group("issue-group").await.expect("seed");
        let entry = scratch.groups().get(&seeded.group_id).await.expect("entry");

        add_issue(
            scratch.backend(),
            &entry,
            AddSpec {
                slug: Some("u-issue".into()),
                title: "Before".into(),
                description: "unchanged".into(),
                body: "body".into(),
                ..AddSpec::default()
            },
            scratch.author(),
        )
        .await
        .expect("seed");

        let updated = update_issue(
            scratch.backend(),
            &entry,
            "u-issue",
            UpdateSpec {
                status: Some(IssueStatus::Wontfix),
                ..UpdateSpec::default()
            },
            scratch.author(),
        )
        .await
        .expect("update");
        assert_eq!(updated.status, IssueStatus::Wontfix);
        assert_eq!(updated.title, "Before");
        assert_eq!(updated.description, "unchanged");
        assert_eq!(updated.body, "body");
    }

    #[tokio::test]
    async fn list_default_hides_closed_and_wontfix() {
        let scratch = ScratchHome::new().await.expect("scratch home");
        let seeded = scratch.seed_group("issue-group").await.expect("seed");
        let entry = scratch.groups().get(&seeded.group_id).await.expect("entry");

        for (slug, status) in [
            ("issue-open", IssueStatus::Open),
            ("issue-closed", IssueStatus::Closed),
            ("issue-blocked", IssueStatus::Blocked),
            ("issue-wontfix", IssueStatus::Wontfix),
        ] {
            add_issue(
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

        let visible = list_issues(scratch.backend(), &entry, None, false)
            .await
            .expect("list default");
        let slugs: Vec<&str> = visible.iter().map(|r| r.slug.as_str()).collect();
        assert!(slugs.contains(&"issue-open"));
        assert!(slugs.contains(&"issue-blocked"));
        assert!(!slugs.contains(&"issue-closed"));
        assert!(!slugs.contains(&"issue-wontfix"));

        let everything = list_issues(scratch.backend(), &entry, None, true)
            .await
            .expect("list all");
        assert_eq!(everything.len(), 4);
    }

    #[tokio::test]
    async fn delete_refuses_when_slug_is_not_an_issue() {
        let scratch = ScratchHome::new().await.expect("scratch home");
        let seeded = scratch.seed_group("issue-group").await.expect("seed");
        let entry = scratch.groups().get(&seeded.group_id).await.expect("entry");

        crate::features::add_feature(
            scratch.backend(),
            &entry,
            crate::features::AddSpec {
                slug: Some("a-feat".into()),
                title: "feat".into(),
                description: "guard test".into(),
                body: "x".into(),
                ..crate::features::AddSpec::default()
            },
            scratch.author(),
        )
        .await
        .expect("seed feat");

        let err = delete_issue(scratch.backend(), &entry, "a-feat", scratch.author(), None)
            .await
            .expect_err("delete must reject");
        assert!(matches!(err, IssueError::NotAnIssue { .. }));
    }

    #[tokio::test]
    async fn rename_moves_under_new_slug() {
        let scratch = ScratchHome::new().await.expect("scratch home");
        let seeded = scratch.seed_group("issue-group").await.expect("seed");
        let entry = scratch.groups().get(&seeded.group_id).await.expect("entry");

        add_issue(
            scratch.backend(),
            &entry,
            AddSpec {
                slug: Some("old-slug".into()),
                title: "Issue".into(),
                description: "rename test".into(),
                body: "x".into(),
                ..AddSpec::default()
            },
            scratch.author(),
        )
        .await
        .expect("seed");

        rename_issue(
            scratch.backend(),
            &entry,
            "old-slug",
            "new-slug",
            scratch.author(),
            None,
        )
        .await
        .expect("rename");

        let loaded = read_issue(scratch.backend(), &entry, "new-slug", None)
            .await
            .expect("read");
        assert_eq!(loaded.slug, "new-slug");
    }
}
