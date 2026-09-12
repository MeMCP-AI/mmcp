//! Typed CRUD over issue tracker memories.
//!
//! Sister surface to [`crate::features`].
//! Wraps the generic memory layer with issue-aware semantics: every write / read commits a memory
//! whose [`MemoryKind`] is `Issue`, carrying a structured [`IssueMetadata`] block in frontmatter.
//! Cross-references take any UUID, so an issue may depend on a feature, another issue, or any future tracker kind.
//! The shared ticket counter lives in [`crate::tracker`],
//! so feature and issue numbers occupy one per-group monotonic sequence (GitHub-style).
//!
//! The hybrid model permits a memory to carry both a `[feature]` and an `[issue]` block.
//! This module's create path always writes a pure-issue memory;
//! hybrid promotion is a future workflow on top of the existing memory editor surface.
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

use crate::diagnostics::Finding;
use crate::groups::GroupEntry;
use crate::home::ResolvedAuthor;
use crate::memory::{
    AddressingMode, ImportError, WriteFileOptions, WriteMemoryOptions, delete_file_at_path,
    resolve_commit_message, resolve_memory, slugify_filename, validate_memory_slug,
    write_file_at_path, write_memory_by_id,
};

/// Errors specific to issue-tracker operations.
#[derive(Debug, thiserror::Error)]
pub enum IssueError {
    /// Propagated from the memory CRUD primitives.
    #[error(transparent)]
    Memory(#[from] ImportError),

    /// Raised when `read_issue` / `update_issue` / `delete_issue` target a memory that exists
    /// but is neither an `Issue` kind nor a hybrid memory carrying an `[issue]` block.
    /// Keeps the issue tools from silently operating on unrelated memories.
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

    /// `update_issue` was called with every mutator field in
    /// [`UpdateSpec`] absent or empty. Caught before `update_issue`
    /// touches the lock chain or the backend, so no read, write, or
    /// commit is ever attempted for the no-op call.
    #[error("update_issue requires at least one mutator field; all fields were omitted or empty")]
    NoChangesSupplied,
}

/// Input for [`add_issue`].
#[derive(Debug, Clone, Default)]
pub struct AddSpec {
    pub slug: Option<String>,
    pub title: String,
    pub description: String,
    pub body: String,
    pub status: IssueStatus,
    /// Optional explicit number override.
    /// Leave absent to let `add_issue` mint the next sequential number from the shared tracker counter.
    pub number: Option<u32>,
    pub depends_on: Vec<Uuid>,
    pub blocks: Vec<Uuid>,
    pub refs: Vec<MemoryRef>,
    /// Slug or UUID of an existing issue in the same project group to supersede.
    /// Cross-kind targets (a feature) are not resolved here in v1;
    /// that capability is staged for a later slice.
    pub supersedes: Option<String>,
    /// Provenance UUID.
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

impl UpdateSpec {
    /// True when at least one mutator field is set.
    /// `message` only overrides the commit message text and never
    /// counts as a change on its own.
    fn has_any_change(&self) -> bool {
        self.title.is_some()
            || self.description.is_some()
            || self.body.is_some()
            || self.status.is_some()
            || self.depends_on.is_some()
            || self.blocks.is_some()
            || self.refs_add.is_some()
            || self.refs_remove.is_some()
            || self.superseded_by.is_some()
    }
}

/// Typed return shape for every read / write path on the issue surface.
/// Mirrors the on-disk frontmatter closely so downstream consumers
/// (CLI formatter, MCP JSON serializer, future issue-bridge) have one canonical shape to convert from.
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

/// Create a new issue in the group.
/// Errors with `IssueError::Memory(ImportError::MemoryAlreadyExists)` when the slug already points at something on disk.
///
/// When `spec.supersedes` is set, runs the two-commit supersede flow against the resolved target
/// (same kind, same group only in v1).
pub async fn add_issue(
    backend: &NativeBackend,
    entry: &GroupEntry,
    spec: AddSpec,
    author: &ResolvedAuthor,
) -> Result<IssueRecord, IssueError> {
    let group = *entry.manifest.group_id.as_uuid();
    let _guards = crate::lock::acquire_chain(&crate::lock::create_chain(group)).await;

    if spec.title.trim().is_empty() && spec.slug.is_none() {
        return Err(IssueError::TitleRequired);
    }
    let slug = match spec.slug.clone() {
        Some(raw) => raw,
        None => slugify_filename(&spec.title),
    };
    validate_memory_slug(&slug).map_err(IssueError::Memory)?;

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
        WriteMemoryOptions {
            addressing_mode: AddressingMode::BySlugOnly,
            message: Some(&message),
            ..Default::default()
        },
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
        IssueStatus::Open | IssueStatus::Blocked | IssueStatus::Deferred | IssueStatus::Closed => {}
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
    validate_memory_slug(slug).map_err(IssueError::Memory)?;
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

/// Update an issue by slug.
/// Public wrapper acquiring the per-group lock chain; delegates to [`update_issue_unlocked`].
pub async fn update_issue(
    backend: &NativeBackend,
    entry: &GroupEntry,
    slug: &str,
    spec: UpdateSpec,
    author: &ResolvedAuthor,
) -> Result<IssueRecord, IssueError> {
    if !spec.has_any_change() {
        return Err(IssueError::NoChangesSupplied);
    }
    let group = *entry.manifest.group_id.as_uuid();
    let _ancestors = crate::lock::acquire_chain(&[
        (
            crate::lock::LockScope::Process,
            crate::lock::LockMode::Shared,
        ),
        (
            crate::lock::LockScope::Group(group),
            crate::lock::LockMode::Shared,
        ),
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
    let current_frontmatter = crate::tracker::read_memory_frontmatter(
        backend,
        &entry.handle,
        &resolved.path,
        IssueError::Memory,
    )
    .await?;

    let title = spec.title.unwrap_or(current.title);
    let description = spec.description.unwrap_or(current.description);
    let body = spec.body.unwrap_or(current.body);
    let status = spec.status.unwrap_or(current.status);
    // Numbers are immutable after create; preserve the on-disk value.
    let number = current.number;
    let depends_on = spec.depends_on.unwrap_or(current.depends_on);
    let blocks = spec.blocks.unwrap_or(current.blocks);
    let refs = crate::tracker::compose_refs(
        current_frontmatter.refs.clone(),
        spec.refs_remove.as_deref(),
        spec.refs_add.as_deref(),
    );
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
    file.frontmatter = crate::tracker::carry_forward_frontmatter(
        file.frontmatter.clone().with_id(resolved.id),
        &current_frontmatter,
        Some(refs),
    );
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
        WriteFileOptions {
            addressing_mode: resolved.addressing_mode,
            message: Some(&message),
            ..Default::default()
        },
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

/// Rename every issue under `old_slug` to `new_slug` in one atomic commit.
/// UUIDs stay stable across the rename.
/// An explicit `message` override is bounded via [`resolve_commit_message`].
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
    validate_memory_slug(old_slug).map_err(IssueError::Memory)?;
    validate_memory_slug(new_slug).map_err(IssueError::Memory)?;
    if old_slug == new_slug {
        return list_issues_for_slug(backend, entry, old_slug).await;
    }

    // Refuse rename when a source memory does not carry an [issue] block.
    // Mirrors the feature side's not-a-feature guard.
    let planned = crate::tracker::plan_slug_rename(backend, entry, old_slug, new_slug, |file| {
        if file.frontmatter.issue.is_none() {
            Some(IssueError::NotAnIssue {
                slug: old_slug.to_string(),
                kind: file.frontmatter.kind.as_str().to_string(),
            })
        } else {
            None
        }
    })
    .await?;

    let commit_message =
        resolve_commit_message(message, || format!("rename issue {old_slug} -> {new_slug}"))
            .map_err(IssueError::Memory)?;
    backend
        .write_commit(
            &entry.handle,
            mmcp_git::CommitSpec::mmcp_commit(commit_message, planned, &author.name, &author.email),
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
    let count = crate::tracker::count_slug_entries(backend, entry, slug)
        .await
        .map_err(IssueError::Memory)?;
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        out.push(read_issue(backend, entry, slug, None).await?);
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
        (
            crate::lock::LockScope::Process,
            crate::lock::LockMode::Shared,
        ),
        (
            crate::lock::LockScope::Group(group),
            crate::lock::LockMode::Shared,
        ),
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
/// `kind` discriminator.
/// Hybrid memories appear in both `list_features` and `list_issues`.
///
/// A memory that IS an issue but whose frontmatter fails to parse is NOT skipped silently:
/// it is excluded from the returned records, a mis-parsed record cannot be trusted,
/// but reported back as a [`Finding`] (`frontmatter_parse_failed`),
/// so callers can surface it through the notes channel,
/// instead of the listing quietly lying about the group's true issue count.
/// `list_features` applies the same rule.
pub async fn list_issues(
    backend: &NativeBackend,
    entry: &GroupEntry,
    status_filter: Option<IssueStatus>,
    show_all: bool,
) -> Result<(Vec<IssueRecord>, Vec<Finding>), IssueError> {
    // One batched walk-and-read instead of a `list_tree` + `read_file` pair per slug:
    // see `crate::tracker::read_all_slug_files` for the O(2N) -> O(1) rationale.
    let files = crate::tracker::read_all_slug_files(backend, entry, Rev::head())
        .await
        .map_err(IssueError::Memory)?;

    let mut out = Vec::new();
    let mut findings = Vec::new();
    for (slug, outcome) in files {
        match outcome {
            Ok(file) => match record_from_file(&slug, file, String::new()) {
                Ok(record) => {
                    if crate::tracker::listing_keeps_status(record.status, status_filter, show_all)
                    {
                        out.push(record);
                    }
                }
                // `NotAnIssue` is an *expected* non-match:
                // the slug is a rule/snapshot/log/reference/scratch/pure feature memory, not a corruption signal.
                Err(IssueError::NotAnIssue { .. }) => {}
                Err(other) => return Err(other),
            },
            // A genuine parse error does NOT silently drop the memory from view:
            // it is surfaced as a finding so a corrupt-on-disk issue is loud instead of invisible.
            Err(ImportError::Parse(err)) => {
                findings.push(crate::tracker::parse_failed_finding(
                    &entry.manifest.group_id.to_string(),
                    &slug,
                    &err,
                ));
            }
            Err(other) => return Err(IssueError::Memory(other)),
        }
    }
    out.sort_by(|a, b| match (a.number, b.number) {
        (Some(x), Some(y)) => x.cmp(&y),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => a.slug.cmp(&b.slug),
    });
    Ok((out, findings))
}

/// Body-free counterpart to [`list_issues`] for listing surfaces.
/// Also forwards `list_issues`'s per-memory parse-error findings unchanged,
/// so callers surface them through the notes channel.
pub async fn list_issue_summaries(
    backend: &NativeBackend,
    entry: &GroupEntry,
    status_filter: Option<IssueStatus>,
    show_all: bool,
) -> Result<(Vec<IssueSummary>, Vec<Finding>), IssueError> {
    let (records, findings) = list_issues(backend, entry, status_filter, show_all).await?;
    let summaries = records.into_iter().map(IssueSummary::from_record).collect();
    Ok((summaries, findings))
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

/// Gates on `[issue]` block PRESENCE, not `frontmatter.kind`, via [`crate::tracker::require_block`]:
/// a hybrid memory (kind=Feature carrying both `[feature]` and `[issue]` blocks) is a real issue
/// per kind.rs's documented hybrid model. Mirrors `features::record_from_file`'s gating exactly.
fn record_from_file(
    slug: &str,
    file: MemoryFile,
    commit_id: String,
) -> Result<IssueRecord, IssueError> {
    let kind = file.frontmatter.kind.as_str().to_string();
    let metadata =
        crate::tracker::require_block(slug, &kind, file.frontmatter.issue, |slug, kind| {
            IssueError::NotAnIssue { slug, kind }
        })?;
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
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use crate::memory::import_memory;
    use crate::testing::ScratchHome;
    use mmcp_core::memory::{BumpIntent, FeatureMetadata, FeatureStatus};

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

    /// Regression guard for the frontmatter-reset defect: `update_issue_unlocked` used to rebuild
    /// its frontmatter from `MemoryFrontmatter::new`'s defaults, silently resetting `tags`,
    /// `mandatory`, `bump_intent`, and `source` on any update, even one naming only `status`.
    /// Asserts the requirement (a field the mutator does not name survives), not a value merely
    /// observed off the pre-fix code.
    #[tokio::test]
    async fn update_preserves_frontmatter_fields_it_does_not_own() {
        let scratch = ScratchHome::new().await.expect("scratch home");
        let seeded = scratch.seed_group("issue-group").await.expect("seed");
        let entry = scratch.groups().get(&seeded.group_id).await.expect("entry");

        let source_id = Uuid::now_v7();
        let seeded_file = MemoryFile {
            frontmatter: MemoryFrontmatter::new("Before", "unchanged", MemoryKind::Issue)
                .with_issue(IssueMetadata {
                    status: IssueStatus::Open,
                    number: Some(1),
                    ..IssueMetadata::default()
                })
                .with_tags(vec!["alpha".to_string(), "beta".to_string()])
                .with_mandatory(true)
                .with_bump_intent(Some(BumpIntent::Patch))
                .with_source(Some(source_id))
                .with_version(Some("1.2.3".parse().expect("valid semver literal"))),
            body: "body".to_string(),
            format: FrontmatterFormat::TomlPlus,
        };
        let seeded_version = seeded_file.frontmatter.version.clone();
        import_memory(
            scratch.backend(),
            &entry.handle,
            "tagged-issue",
            &seeded_file.to_string().expect("render seeded issue"),
            None,
            scratch.author(),
            false,
        )
        .await
        .expect("seed tagged issue");

        let assert_carried_forward = |frontmatter: &MemoryFrontmatter| {
            assert_eq!(
                frontmatter.tags,
                vec!["alpha".to_string(), "beta".to_string()],
                "an update naming only one field must not reset tags"
            );
            assert!(
                frontmatter.mandatory,
                "an update naming only one field must not reset mandatory"
            );
            assert_eq!(
                frontmatter.bump_intent,
                Some(BumpIntent::Patch),
                "an update naming only one field must not reset bump_intent"
            );
            assert_eq!(
                frontmatter.source,
                Some(source_id),
                "an update naming only one field must not reset source"
            );
            assert_eq!(
                frontmatter.version, seeded_version,
                "an update naming only one field must not reset version"
            );
        };

        update_issue(
            scratch.backend(),
            &entry,
            "tagged-issue",
            UpdateSpec {
                status: Some(IssueStatus::Wontfix),
                ..UpdateSpec::default()
            },
            scratch.author(),
        )
        .await
        .expect("status-only update");
        let resolved = resolve_memory(scratch.backend(), &entry.handle, Some("tagged-issue"), None)
            .await
            .expect("resolve after status-only update");
        let frontmatter = crate::tracker::read_memory_frontmatter(
            scratch.backend(),
            &entry.handle,
            &resolved.path,
            IssueError::Memory,
        )
        .await
        .expect("read frontmatter after status-only update");
        assert_carried_forward(&frontmatter);

        update_issue(
            scratch.backend(),
            &entry,
            "tagged-issue",
            UpdateSpec {
                description: Some("a different unrelated description".into()),
                ..UpdateSpec::default()
            },
            scratch.author(),
        )
        .await
        .expect("description-only update");
        let resolved = resolve_memory(scratch.backend(), &entry.handle, Some("tagged-issue"), None)
            .await
            .expect("resolve after description-only update");
        let frontmatter = crate::tracker::read_memory_frontmatter(
            scratch.backend(),
            &entry.handle,
            &resolved.path,
            IssueError::Memory,
        )
        .await
        .expect("read frontmatter after description-only update");
        assert_carried_forward(&frontmatter);
    }

    /// Regression guard for the hybrid sibling-block defect: a hybrid memory (kind=Feature,
    /// carrying both a `[feature]` and an `[issue]` block, per `kind.rs`'s documented hybrid
    /// model) must keep its `feature` block and its primary `kind` intact when `update_issue`
    /// only touches the `[issue]` block's status.
    #[tokio::test]
    async fn update_on_a_hybrid_record_preserves_the_sibling_feature_block_and_kind() {
        let scratch = ScratchHome::new().await.expect("scratch home");
        let seeded = scratch.seed_group("issue-group").await.expect("seed");
        let entry = scratch.groups().get(&seeded.group_id).await.expect("entry");

        let feature_block = FeatureMetadata {
            status: FeatureStatus::Requested,
            number: Some(1),
            ..FeatureMetadata::default()
        };
        let seeded_file = MemoryFile {
            frontmatter: MemoryFrontmatter::new("Hybrid", "both blocks", MemoryKind::Feature)
                .with_feature(feature_block.clone())
                .with_issue(IssueMetadata {
                    status: IssueStatus::Open,
                    number: Some(1),
                    ..IssueMetadata::default()
                }),
            body: "body".to_string(),
            format: FrontmatterFormat::TomlPlus,
        };
        import_memory(
            scratch.backend(),
            &entry.handle,
            "hybrid-ticket",
            &seeded_file.to_string().expect("render seeded hybrid"),
            None,
            scratch.author(),
            false,
        )
        .await
        .expect("seed hybrid ticket");

        update_issue(
            scratch.backend(),
            &entry,
            "hybrid-ticket",
            UpdateSpec {
                status: Some(IssueStatus::Wontfix),
                ..UpdateSpec::default()
            },
            scratch.author(),
        )
        .await
        .expect("status-only update on hybrid record");

        let resolved = resolve_memory(
            scratch.backend(),
            &entry.handle,
            Some("hybrid-ticket"),
            None,
        )
        .await
        .expect("resolve after update");
        let frontmatter = crate::tracker::read_memory_frontmatter(
            scratch.backend(),
            &entry.handle,
            &resolved.path,
            IssueError::Memory,
        )
        .await
        .expect("read frontmatter after update");

        assert_eq!(
            frontmatter.kind,
            MemoryKind::Feature,
            "update_issue must not flip a hybrid record's primary kind"
        );
        assert_eq!(
            frontmatter.feature,
            Some(feature_block),
            "update_issue must not drop the sibling feature block on a hybrid record"
        );
        assert_eq!(
            frontmatter.issue.map(|m| m.status),
            Some(IssueStatus::Wontfix),
            "the named field (issue status) must still apply"
        );
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

        let (visible, findings) = list_issues(scratch.backend(), &entry, None, false)
            .await
            .expect("list default");
        assert!(findings.is_empty());
        let slugs: Vec<&str> = visible.iter().map(|r| r.slug.as_str()).collect();
        assert!(slugs.contains(&"issue-open"));
        assert!(slugs.contains(&"issue-blocked"));
        assert!(!slugs.contains(&"issue-closed"));
        assert!(!slugs.contains(&"issue-wontfix"));

        let (everything, _findings) = list_issues(scratch.backend(), &entry, None, true)
            .await
            .expect("list all");
        assert_eq!(everything.len(), 4);
    }

    #[tokio::test]
    async fn list_issues_surfaces_parse_error_as_finding_not_silent_drop() {
        // Regression test for the swallowed-parse-error bug mirrored
        // from features.rs: a corrupt issue memory must not simply
        // vanish from the listing with no trace.
        let scratch = ScratchHome::new().await.expect("scratch home");
        let seeded = scratch.seed_group("issue-group").await.expect("seed");
        let entry = scratch.groups().get(&seeded.group_id).await.expect("entry");

        add_issue(
            scratch.backend(),
            &entry,
            AddSpec {
                slug: Some("good".into()),
                title: "Good".into(),
                ..AddSpec::default()
            },
            scratch.author(),
        )
        .await
        .expect("seed good issue");

        // Seed a memory whose frontmatter is a genuine parse
        // failure (no `+++` fences at all), not merely an
        // edge-case-but-valid value.
        let bad_id = mmcp_core::id::MemoryId::new();
        let author = scratch.author();
        scratch
            .backend()
            .write_commit(
                &entry.handle,
                mmcp_git::CommitSpec::mmcp_commit(
                    "seed corrupt/bad".to_string(),
                    vec![(
                        mmcp_core::conventions::memory_path("corrupt", bad_id),
                        Some(b"not a memory file at all\n".to_vec()),
                    )],
                    &author.name,
                    &author.email,
                ),
            )
            .await
            .expect("seed corrupt memory");

        let (records, findings) = list_issues(scratch.backend(), &entry, None, true)
            .await
            .expect("list must not fail the whole group over one corrupt memory");

        let slugs: Vec<_> = records.iter().map(|r| r.slug.as_str()).collect();
        assert_eq!(
            slugs,
            vec!["good"],
            "the corrupt memory must not appear as a trustworthy record",
        );

        assert_eq!(
            findings.len(),
            1,
            "the corrupt memory must be reported, not silently dropped",
        );
        assert_eq!(findings[0].code, "frontmatter_parse_failed");
        assert_eq!(findings[0].slug.as_deref(), Some("corrupt"));
    }

    /// `list_issues` reads every slug through one batched
    /// `tracker::read_all_slug_files` call (one `list_memory_slug_dirs`
    /// walk plus one batched `read_files`) instead of a `list_tree` +
    /// `read_file` pair per slug. Seeding enough issues to span many
    /// slug directories confirms the batched path returns exactly the
    /// same records a per-slug loop would, not merely that it compiles.
    #[tokio::test]
    async fn list_issues_batches_many_issues_correctly() {
        let scratch = ScratchHome::new().await.expect("scratch home");
        let seeded = scratch.seed_group("issue-group").await.expect("seed");
        let entry = scratch.groups().get(&seeded.group_id).await.expect("entry");

        const ISSUE_COUNT: u32 = 25;
        for i in 0..ISSUE_COUNT {
            add_issue(
                scratch.backend(),
                &entry,
                AddSpec {
                    slug: Some(format!("bulk-issue-{i}")),
                    title: format!("Bulk issue {i}"),
                    description: "bulk listing test".into(),
                    body: "b".into(),
                    status: IssueStatus::Open,
                    ..AddSpec::default()
                },
                scratch.author(),
            )
            .await
            .expect("seed bulk issue");
        }

        let (records, findings) = list_issues(scratch.backend(), &entry, None, true)
            .await
            .expect("list all bulk issues");
        assert!(findings.is_empty());
        assert_eq!(records.len(), ISSUE_COUNT as usize);
        let numbers: Vec<u32> = records.iter().filter_map(|r| r.number).collect();
        let expected: Vec<u32> = (1..=ISSUE_COUNT).collect();
        assert_eq!(
            numbers, expected,
            "listing must stay sorted by number ascending"
        );
        let slugs: std::collections::HashSet<&str> =
            records.iter().map(|r| r.slug.as_str()).collect();
        for i in 0..ISSUE_COUNT {
            assert!(slugs.contains(format!("bulk-issue-{i}").as_str()));
        }
    }

    /// Two UUID-named files under one slug directory reproduce the same
    /// `MemoryAmbiguous` a per-slug `resolve_by_slug` call would raise.
    /// The batched read path aborts the whole listing on it,
    /// instead of silently dropping or partially resolving the ambiguous slug.
    #[tokio::test]
    async fn list_issues_aborts_whole_listing_on_ambiguous_slug() {
        let scratch = ScratchHome::new().await.expect("scratch home");
        let seeded = scratch.seed_group("issue-group").await.expect("seed");
        let entry = scratch.groups().get(&seeded.group_id).await.expect("entry");

        add_issue(
            scratch.backend(),
            &entry,
            AddSpec {
                slug: Some("good".into()),
                title: "Good".into(),
                ..AddSpec::default()
            },
            scratch.author(),
        )
        .await
        .expect("seed good issue");

        let metadata = IssueMetadata {
            status: IssueStatus::Open,
            number: Some(50),
            ..IssueMetadata::default()
        };
        let file = build_memory_file("Ambiguous".into(), "d".into(), "b".into(), metadata);
        let rendered = file.to_string().expect("render ambiguous issue");
        let id_a = mmcp_core::id::MemoryId::new();
        let id_b = mmcp_core::id::MemoryId::new();
        let author = scratch.author();
        scratch
            .backend()
            .write_commit(
                &entry.handle,
                mmcp_git::CommitSpec::mmcp_commit(
                    "seed ambiguous issue".to_string(),
                    vec![
                        (
                            mmcp_core::conventions::memory_path("ambiguous-issue", id_a),
                            Some(rendered.clone().into_bytes()),
                        ),
                        (
                            mmcp_core::conventions::memory_path("ambiguous-issue", id_b),
                            Some(rendered.into_bytes()),
                        ),
                    ],
                    &author.name,
                    &author.email,
                ),
            )
            .await
            .expect("seed ambiguous memory");

        let err = list_issues(scratch.backend(), &entry, None, true)
            .await
            .expect_err("ambiguous slug must abort the whole listing");
        assert!(matches!(
            err,
            IssueError::Memory(ImportError::MemoryAmbiguous { .. })
        ));
    }

    /// A slug directory name that fails `validate_memory_slug` (here,
    /// an uppercase segment) is unreachable through `add_issue`'s own write path.
    /// It can land on disk via direct git surgery or an externally imported repo.
    /// The batched read path aborts the whole listing on it,
    /// identically to `read_issue`'s own `validate_memory_slug` gate.
    #[tokio::test]
    async fn list_issues_aborts_whole_listing_on_invalid_slug_name() {
        let scratch = ScratchHome::new().await.expect("scratch home");
        let seeded = scratch.seed_group("issue-group").await.expect("seed");
        let entry = scratch.groups().get(&seeded.group_id).await.expect("entry");

        add_issue(
            scratch.backend(),
            &entry,
            AddSpec {
                slug: Some("good".into()),
                title: "Good".into(),
                ..AddSpec::default()
            },
            scratch.author(),
        )
        .await
        .expect("seed good issue");

        let metadata = IssueMetadata {
            status: IssueStatus::Open,
            number: Some(51),
            ..IssueMetadata::default()
        };
        let file = build_memory_file("Bad slug".into(), "d".into(), "b".into(), metadata);
        let rendered = file.to_string().expect("render bad-slug issue");
        let bad_id = mmcp_core::id::MemoryId::new();
        let author = scratch.author();
        scratch
            .backend()
            .write_commit(
                &entry.handle,
                mmcp_git::CommitSpec::mmcp_commit(
                    "seed invalid-slug issue".to_string(),
                    vec![(
                        mmcp_core::conventions::memory_path("Bad_Slug", bad_id),
                        Some(rendered.into_bytes()),
                    )],
                    &author.name,
                    &author.email,
                ),
            )
            .await
            .expect("seed invalid-slug memory");

        let err = list_issues(scratch.backend(), &entry, None, true)
            .await
            .expect_err("invalid slug name must abort the whole listing");
        assert!(matches!(
            err,
            IssueError::Memory(ImportError::InvalidSlug(_))
        ));
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

    #[tokio::test]
    async fn rename_issue_rejects_oversized_message() {
        let scratch = ScratchHome::new().await.expect("scratch home");
        let seeded = scratch.seed_group("issue-group").await.expect("seed");
        let entry = scratch.groups().get(&seeded.group_id).await.expect("entry");

        add_issue(
            scratch.backend(),
            &entry,
            AddSpec {
                slug: Some("msg-src".into()),
                title: "Issue".into(),
                description: "rename message test".into(),
                body: "x".into(),
                ..AddSpec::default()
            },
            scratch.author(),
        )
        .await
        .expect("seed");

        let oversized = "a".repeat(mmcp_core::memory::MAX_MESSAGE_LENGTH + 1);
        let err = rename_issue(
            scratch.backend(),
            &entry,
            "msg-src",
            "msg-dst",
            scratch.author(),
            Some(&oversized),
        )
        .await
        .expect_err("oversized message rejected");
        assert!(matches!(
            err,
            IssueError::Memory(ImportError::FieldTooLong(_))
        ));
        // Rejected before any commit: the issue is still at its
        // original slug.
        read_issue(scratch.backend(), &entry, "msg-src", None)
            .await
            .expect("still at original slug");
    }

    #[tokio::test]
    async fn rename_issue_accepts_message_within_bound() {
        let scratch = ScratchHome::new().await.expect("scratch home");
        let seeded = scratch.seed_group("issue-group").await.expect("seed");
        let entry = scratch.groups().get(&seeded.group_id).await.expect("entry");

        add_issue(
            scratch.backend(),
            &entry,
            AddSpec {
                slug: Some("msg-src-ok".into()),
                title: "Issue".into(),
                description: "rename message test".into(),
                body: "x".into(),
                ..AddSpec::default()
            },
            scratch.author(),
        )
        .await
        .expect("seed");

        let bounded = "a".repeat(mmcp_core::memory::MAX_MESSAGE_LENGTH);
        rename_issue(
            scratch.backend(),
            &entry,
            "msg-src-ok",
            "msg-dst-ok",
            scratch.author(),
            Some(&bounded),
        )
        .await
        .expect("bounded message accepted");

        let loaded = read_issue(scratch.backend(), &entry, "msg-dst-ok", None)
            .await
            .expect("read");
        assert_eq!(loaded.slug, "msg-dst-ok");
    }
}
