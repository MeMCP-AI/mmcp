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
    MemoryRef,
};
use mmcp_git::{GitBackend, NativeBackend, Rev};
use uuid::Uuid;

use crate::config::{find_project_root, load as load_project_config};
use crate::groups::{GroupEntry, GroupIndex};
use crate::home::ResolvedAuthor;
use crate::memory::{
    AddressingMode, ImportError, WriteFileOptions, WriteMemoryOptions, delete_file_at_path,
    resolve_commit_message, resolve_memory, slugify_filename, validate_slug, write_file_at_path,
    write_memory_by_id,
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

    /// Malformed cross-reference input parsed through
    /// [`mmcp_core::memory::xrefs`]. Surfaces both the
    /// `InvalidCrossRef` and `InvalidMemoryRef` cases so
    /// `map_feature_error_to_mcp` and the CLI handlers can
    /// pattern-match through one variant on the feature side
    /// without losing the field-attribution detail the parser
    /// recorded.
    #[error(transparent)]
    Xref(#[from] mmcp_core::memory::XrefError),

    /// `supersedes` pointed at a slug / UUID the local mirror could
    /// not resolve in the caller's project group.
    #[error("supersedes target '{query}' does not resolve in this project group")]
    SupersedesUnknown { query: String },

    /// `supersedes` pointed at an FR whose current status rules out
    /// supersession: `Resolved`, `Duplicate`, or `Superseded`. The
    /// already-superseded case carries the typed back-link so
    /// callers can chase to the tip of the chain.
    #[error("supersedes target '{slug}' has status '{}' which cannot be superseded", status.as_str())]
    SupersedesInvalidStatus {
        slug: String,
        status: FeatureStatus,
        /// When `status == Superseded`, the `superseded_by` link on
        /// the target so callers can follow the chain without a
        /// second round-trip. `None` on every other bad status.
        existing_link: Option<MemoryRef>,
    },

    /// `supersedes` pointed at an FR in a different group. v1 rejects
    /// cross-group supersede until the project-selector FR lands a
    /// broader story; the data model itself supports it.
    #[error(
        "supersedes target '{query}' lives in a different group than the caller's project group; cross-group supersede is not yet supported"
    )]
    SupersedesCrossGroupUnsupported { query: String },

    /// An explicit `project` selector did not resolve against the
    /// local mirror. Covers FR-44's `unknown_project` code: the
    /// caller passed a UUID or slug that is not a mirrored group.
    #[error("project selector '{query}' does not resolve to a mirrored group")]
    UnknownProject { query: String },
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
    /// Typed cross-references attached to the new FR. When
    /// [`AddSpec::supersedes`] is set and this is empty, the
    /// supersede flow auto-populates one entry pointing at the
    /// old FR at its pre-supersede commit. Callers that want an
    /// explicit refs list plus the auto-entry should include the
    /// old-FR ref themselves; the flow dedupes by `target`.
    pub refs: Vec<MemoryRef>,
    /// Slug or UUID (string form) of an existing FR in the same
    /// project group to supersede. When present, `add_feature`
    /// runs the two-commit supersede flow: commit A writes the
    /// new FR, commit B re-writes the old FR with
    /// `status = Superseded` and `superseded_by` pointing at the
    /// new FR's commit A.
    pub supersedes: Option<String>,
    /// FR-38 provenance. When set, this FR was filed by an agent
    /// acting on behalf of the named owner — group UUID for
    /// federated workflows, memory UUID when promoted from an
    /// existing reference memory. Stamped into the frontmatter at
    /// commit time and never re-resolved.
    pub source: Option<Uuid>,
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
    pub depends_on: Option<Vec<Uuid>>,
    pub blocks: Option<Vec<Uuid>>,
    /// Compose-dedup add-side for `refs`. Entries with the same
    /// `target` as an existing ref replace it (commit sha wins
    /// from the add side); truly new entries are appended.
    pub refs_add: Option<Vec<MemoryRef>>,
    /// Compose-dedup remove-side for `refs`. Removes every
    /// existing ref whose `target` matches any uuid in this list,
    /// ignoring commit sha so callers do not have to remember
    /// which revision a ref was pinned to.
    pub refs_remove: Option<Vec<Uuid>>,
    /// Typed back-link retry path for the two-commit supersede
    /// flow. Set when commit B of [`add_feature`]'s supersede
    /// needs to be re-run after a partial landing:
    /// `update_feature(old_slug, UpdateSpec { status:
    /// Superseded, superseded_by: Some(ref), ..default })`.
    /// `None` leaves the existing back-link untouched.
    pub superseded_by: Option<MemoryRef>,
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
    /// Typed back-link to the FR that replaced this one. Present
    /// when the FR has been through the supersede flow; `None`
    /// otherwise. Paired with `status = FeatureStatus::Superseded`
    /// at the frontmatter level via
    /// [`FeatureMetadata::validate_supersede_invariant`].
    pub superseded_by: Option<MemoryRef>,
    /// Commit id of the most recent write for this FR, or the head
    /// commit that produced the record on a read. Empty string on a
    /// freshly read FR whose history starts before this field was
    /// introduced — the field is a convenience for telemetry, not a
    /// correctness primitive.
    pub commit_id: String,
}

/// Body-free projection of a [`FeatureRecord`] for list-style
/// surfaces.
///
/// Mirrors every frontmatter-derived field of `FeatureRecord` and
/// drops `body`. Listings (`list_feature_summaries`, the MCP
/// `list_features` tool, the `mmcp feature list` CLI subcommand)
/// only need metadata to triage / sort / display — keeping bodies
/// out of the wire shape stops a 47-FR group from blowing past the
/// MCP client's response token cap.
#[derive(Debug, Clone)]
pub struct FeatureSummary {
    pub slug: String,
    pub title: String,
    pub description: String,
    pub status: FeatureStatus,
    pub number: Option<u32>,
    pub depends_on: Vec<Uuid>,
    pub blocks: Vec<Uuid>,
    pub superseded_by: Option<MemoryRef>,
    pub commit_id: String,
}

impl FeatureSummary {
    /// Project a full record onto its body-free summary view.
    /// Used by the interim `list_feature_summaries` impl that still
    /// reads bodies; the FR-049 frontmatter-only primitive will
    /// build summaries directly without ever materialising a body.
    fn from_record(record: FeatureRecord) -> Self {
        let FeatureRecord {
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

/// Create a new FR in the group. Errors with
/// `FeatureError::Memory(ImportError::MemoryAlreadyExists)` when
/// the slug already points at something on disk, mirroring the
/// strict-create contract the rest of the memory surface enforces.
///
/// When `spec.supersedes` is set, the call runs the two-commit
/// supersede flow:
///
/// 1. Resolve the old FR in the same group. Reject when the old
///    FR is in a status that cannot be superseded (`Resolved`,
///    `Duplicate`, `Superseded`).
/// 2. Commit A: write the new FR. Its `refs` list auto-receives
///    an entry pointing at the old FR pinned to that FR's
///    pre-supersede HEAD commit.
/// 3. Commit B: re-write the old FR with `status = Superseded`
///    and `superseded_by` pointing at commit A.
///
/// Commits A and B are sequential. The inconsistency window
/// between them is small and recoverable: if B fails, callers
/// complete the chain with `update_feature(old_slug, UpdateSpec {
/// status: Some(Superseded), superseded_by: Some(ref), .. })`
/// carrying the new FR's ref.
pub async fn add_feature(
    backend: &NativeBackend,
    entry: &GroupEntry,
    spec: AddSpec,
    author: &ResolvedAuthor,
) -> Result<FeatureRecord, FeatureError> {
    // FR-39 v2: group-level create — Exclusive on Group(g)
    // serialises `next_feature_number` against every concurrent
    // create in the same group regardless of kind. The supersede
    // flow's `update_feature_unlocked` call below stays inside
    // this guard; the group-exclusive ancestor blocks every
    // nested Memory-leaf write under the same group until we
    // release.
    let group = *entry.manifest.group_id.as_uuid();
    let _guards = crate::lock::acquire_chain(&crate::lock::create_chain(group)).await;

    if spec.title.trim().is_empty() && spec.slug.is_none() {
        return Err(FeatureError::TitleRequired);
    }
    let slug = match spec.slug.clone() {
        Some(raw) => raw,
        None => slugify_filename(&spec.title),
    };
    validate_slug(&slug).map_err(FeatureError::Memory)?;

    // Resolve the old FR up front (before we auto-assign the new
    // number) so supersede-specific errors surface before we touch
    // the numbering state.
    let supersede_target = match spec.supersedes.as_deref() {
        Some(query) => Some(resolve_supersede_target(backend, entry, query).await?),
        None => None,
    };

    // Auto-assign the sequential number when the caller did not
    // pin one. The FR-027 migration binary pins explicitly so
    // historic `fr-NNN-*` numbers are preserved; ordinary creates
    // pick `max(existing) + 1`. Gaps from deletes stay gaps.
    let number = match spec.number {
        Some(n) => Some(n),
        None => Some(next_feature_number(backend, entry).await?),
    };

    let id = Uuid::now_v7();

    // Build the new FR's refs: start with whatever the caller
    // provided, then dedupe-append the auto-entry for the
    // supersede target so callers that explicitly listed the old
    // FR still get one canonical entry.
    let mut refs = spec.refs.clone();
    if let Some(target) = supersede_target.as_ref() {
        let auto_ref = MemoryRef::new(target.id, target.head_commit.clone());
        if !refs.iter().any(|r| r.target == target.id) {
            refs.push(auto_ref);
        }
    }

    let metadata = FeatureMetadata {
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
        .map_err(|e| FeatureError::Memory(ImportError::Render(e.to_string())))?;

    let message = spec
        .message
        .clone()
        .unwrap_or_else(|| match supersede_target.as_ref() {
            Some(t) => format!("create feature {slug} (supersedes {})", t.slug),
            None => format!("create feature {slug}"),
        });
    // FR-28 / D4: feature creation mints `id` and stamps it into
    // frontmatter; filename and frontmatter agree by construction.
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

    // Commit B of the supersede flow: re-write the old FR with
    // `status = Superseded` and `superseded_by` pointing at the
    // commit we just wrote above. A failure here leaves the new
    // FR live but the old one un-superseded; callers recover by
    // retrying `update_feature` with the same knobs.
    //
    // Use the _unlocked variant: `add_feature`'s caller already
    // holds the per-group lock (acquired at the top of this
    // function via the public wrapper below), so re-calling the
    // locked public `update_feature` would deadlock on the
    // non-reentrant mutex.
    if let Some(target) = supersede_target {
        let back_link = MemoryRef::new(id, commit_id.clone());
        let retry_message = format!("mark {} superseded by {slug}", target.slug);
        update_feature_unlocked(
            backend,
            entry,
            &target.slug,
            UpdateSpec {
                status: Some(FeatureStatus::Superseded),
                superseded_by: Some(back_link),
                message: Some(retry_message),
                ..UpdateSpec::default()
            },
            author,
        )
        .await?;
    }

    Ok(FeatureRecord {
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

/// Internal resolved form of an `AddSpec::supersedes` target.
struct SupersedeTarget {
    slug: String,
    id: Uuid,
    head_commit: String,
}

/// Resolve `query` (a slug or UUID string) into a [`SupersedeTarget`]
/// against `entry`'s group, rejecting every status that cannot be
/// superseded.
async fn resolve_supersede_target(
    backend: &NativeBackend,
    entry: &GroupEntry,
    query: &str,
) -> Result<SupersedeTarget, FeatureError> {
    // Simplification for v1: resolve by slug. UUID-by-slug
    // disambiguation lands alongside FR-028's full UUID surface;
    // today the feature tools keep the slug-centric lookup the
    // rest of the FR CRUD uses.
    let record = match read_feature(backend, entry, query, None).await {
        Ok(r) => r,
        Err(FeatureError::Memory(ImportError::MemoryNotFound { .. })) => {
            return Err(FeatureError::SupersedesUnknown {
                query: query.to_string(),
            });
        }
        Err(other) => return Err(other),
    };

    // FR-51: Resolved targets are now allowed so a redesign that
    // replaces a landed feature can capture the relationship as a
    // typed `superseded_by` chain instead of prose-only references.
    // Duplicate keeps its own redirect semantics; Superseded already
    // carries a back-link the caller should chase to the tip.
    match record.status {
        FeatureStatus::Open
        | FeatureStatus::Blocked
        | FeatureStatus::Deferred
        | FeatureStatus::Resolved => {}
        FeatureStatus::Superseded => {
            return Err(FeatureError::SupersedesInvalidStatus {
                slug: record.slug,
                status: FeatureStatus::Superseded,
                existing_link: record.superseded_by,
            });
        }
        other => {
            return Err(FeatureError::SupersedesInvalidStatus {
                slug: record.slug,
                status: other,
                existing_link: None,
            });
        }
    }

    // The FR's UUID is minted into the frontmatter on create (FR-028);
    // every present-era FR has one. Legacy memories that predate FR-028
    // are read via `write_memory_by_id` migrations; if we ever hit one
    // without an id, surface it as an unknown target rather than
    // committing a supersede back-link against an empty UUID.
    let resolved = resolve_memory(backend, &entry.handle, Some(&record.slug), None)
        .await
        .map_err(FeatureError::Memory)?;

    Ok(SupersedeTarget {
        slug: record.slug,
        id: resolved.id,
        head_commit: record.commit_id,
    })
}

/// Internal alias delegating to the shared tracker counter so the
/// feature surface keeps the historical name at its call sites.
/// The real logic lives in `mmcp_store::tracker::next_ticket_number`
/// per global-coding-rules section 13.
async fn next_feature_number(
    backend: &NativeBackend,
    entry: &GroupEntry,
) -> Result<u32, FeatureError> {
    crate::tracker::next_ticket_number(backend, entry)
        .await
        .map_err(FeatureError::Memory)
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
                    id: Some(resolved.id),
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
///
/// Public wrapper: acquires the per-group write lock (FR-39) and
/// delegates to [`update_feature_unlocked`]. Callers that already
/// hold the lock (for instance `add_feature`'s supersede flow)
/// must call `update_feature_unlocked` directly to avoid
/// deadlocking on the non-reentrant mutex.
pub async fn update_feature(
    backend: &NativeBackend,
    entry: &GroupEntry,
    slug: &str,
    spec: UpdateSpec,
    author: &ResolvedAuthor,
) -> Result<FeatureRecord, FeatureError> {
    // FR-39 v2: take ancestor chain Shared, resolve the memory's
    // canonical UUID under that view, then upgrade to Exclusive
    // Memory leaf. Concurrent edits to *different* feature
    // memories in the same group proceed in parallel; only edits
    // to the *same* memory contend.
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
    let resolved = crate::memory::resolve_memory(backend, &entry.handle, Some(slug), None)
        .await
        .map_err(FeatureError::Memory)?;
    let _leaf = crate::lock::acquire(
        crate::lock::LockScope::Memory(resolved.id),
        crate::lock::LockMode::Exclusive,
    )
    .await;
    update_feature_unlocked(backend, entry, slug, spec, author).await
}

/// Inner, non-locking variant of [`update_feature`]. Every caller
/// is responsible for acquiring the per-group lock themselves; the
/// public wrapper does that for external entry points.
pub async fn update_feature_unlocked(
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
    // Pull the current refs list off the on-disk memory so compose
    // ops can merge against it. `read_feature` drops the frontmatter
    // `refs` since it builds a feature-centric record; re-read the
    // raw memory here to preserve them across the update.
    let current_refs = read_memory_refs(backend, &entry.handle, &resolved.path).await?;

    let title = spec.title.unwrap_or(current.title);
    let description = spec.description.unwrap_or(current.description);
    let body = spec.body.unwrap_or(current.body);
    let status = spec.status.unwrap_or(current.status);
    // FR-37: numbers are immutable after create. Preserve whatever
    // the on-disk memory already carries; no UpdateSpec surface for
    // changing it. The one-shot migrations that needed this path
    // were retired with the `migrate_fr_slugs` example.
    let number = current.number;
    let depends_on = spec.depends_on.unwrap_or(current.depends_on);
    let blocks = spec.blocks.unwrap_or(current.blocks);
    // Compose-dedup on the typed refs: remove-side first (by
    // target UUID, ignoring commit), then add-side (dedup by
    // target so add-side wins the commit pin on collision).
    let refs = compose_refs(
        current_refs,
        spec.refs_remove.as_deref(),
        spec.refs_add.as_deref(),
    );
    // `superseded_by`: `Some` replaces, `None` leaves the existing
    // on-disk back-link untouched. Clearing requires a direct
    // frontmatter edit — not yet plumbed to avoid overloading this
    // shape.
    let superseded_by = spec.superseded_by.or(current.superseded_by);

    let metadata = FeatureMetadata {
        status,
        number,
        depends_on: depends_on.clone(),
        blocks: blocks.clone(),
        superseded_by: superseded_by.clone(),
    };
    metadata
        .validate_supersede_invariant()
        .map_err(|e| FeatureError::Memory(ImportError::Render(e.to_string())))?;

    let mut file = build_memory_file(title.clone(), description.clone(), body.clone(), metadata);
    // Preserve the id pinned on disk so the rewrite hits the same
    // canonical path and stays addressable by UUID across the edit.
    file.frontmatter = file
        .frontmatter
        .clone()
        .with_id(resolved.id)
        .with_refs(refs);
    let rendered = file
        .to_string()
        .map_err(|e| FeatureError::Memory(ImportError::Render(e.to_string())))?;

    let message = spec
        .message
        .clone()
        .unwrap_or_else(|| format!("update feature {slug}"));
    // FR-28 / D4: update_feature_unlocked operates on the
    // pre-resolved feature memory; the rendered fm.id is the
    // existing memory's id (preserved through the in-memory edit),
    // so filename and frontmatter agree by construction. Use the
    // resolver's `addressing_mode` and `force=false` so any future
    // drift surfaces through the validation channel rather than
    // bypassing it.
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

    Ok(FeatureRecord {
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

/// Read the raw `refs` list off a memory's frontmatter without
/// going through [`FeatureRecord`] (which deliberately omits
/// general-purpose refs to keep feature-flavored listings focused).
async fn read_memory_refs(
    backend: &NativeBackend,
    handle: &mmcp_git::RepoHandle,
    path: &str,
) -> Result<Vec<MemoryRef>, FeatureError> {
    let bytes = backend
        .read_file(handle, path, &Rev::head())
        .await
        .map_err(|e| FeatureError::Memory(ImportError::Git(e)))?;
    let text = std::str::from_utf8(&bytes)
        .map_err(|e| FeatureError::Memory(ImportError::Render(e.to_string())))?;
    let file = MemoryFile::parse(text).map_err(|e| FeatureError::Memory(ImportError::Parse(e)))?;
    Ok(file.frontmatter.refs)
}

/// Merge an add-side and remove-side edit onto an existing typed
/// refs list. The remove-side runs first by UUID match (commit sha
/// ignored), then the add-side dedupes-and-replaces by target so
/// add-side commit pins win on collision.
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
/// succeed on a non-existent rename. An explicit `message` override
/// is bounded via [`resolve_commit_message`].
pub async fn rename_feature(
    backend: &NativeBackend,
    entry: &GroupEntry,
    old_slug: &str,
    new_slug: &str,
    author: &ResolvedAuthor,
    message: Option<&str>,
) -> Result<Vec<FeatureRecord>, FeatureError> {
    // FR-39 v2: rename always uses `concept:group_coarsening` —
    // Exclusive Group(g) blocks every narrower Shared-Group
    // holder via the ancestor-prefix rule, so no concurrent
    // memory edit can race with a slug-wide move.
    let _guards = crate::lock::acquire_chain(&crate::lock::coarsen_group_chain(
        *entry.manifest.group_id.as_uuid(),
    ))
    .await;
    validate_slug(old_slug).map_err(FeatureError::Memory)?;
    validate_slug(new_slug).map_err(FeatureError::Memory)?;
    if old_slug == new_slug {
        // Explicit short-circuit so operators don't pay a commit
        // for a no-op. A fresh listing is cheap and matches the
        // semantics callers expect from "rename to the same slug".
        return list_features_for_slug(backend, entry, old_slug).await;
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

    let commit_message = resolve_commit_message(message, || {
        format!("rename feature {old_slug} -> {new_slug}")
    })
    .map_err(FeatureError::Memory)?;
    backend
        .write_commit(
            &entry.handle,
            mmcp_git::CommitSpec::mmcp_commit(commit_message, moves, &author.name, &author.email),
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
    // FR-39 v2: same modify chain as `update_feature` — Shared on
    // the ancestor chain, Exclusive on the per-memory leaf.
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
    let resolved = crate::memory::resolve_memory(backend, &entry.handle, Some(slug), None)
        .await
        .map_err(FeatureError::Memory)?;
    let _leaf = crate::lock::acquire(
        crate::lock::LockScope::Memory(resolved.id),
        crate::lock::LockMode::Exclusive,
    )
    .await;
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
/// Filter precedence (FR-024 + supersede follow-up):
/// 1. `status_filter = Some(x)` → include every FR whose status
///    matches, regardless of `show_all`. Explicit selector wins so
///    a caller asking for `resolved` or `superseded` FRs always
///    sees them.
/// 2. `status_filter = None` + `show_all = true` → include every
///    FR. The "show me literally everything" escape hatch.
/// 3. `status_filter = None` + `show_all = false` → hide every
///    status marked [`FeatureStatus::is_default_hidden`]
///    (`Resolved`, `Duplicate`, `Superseded`). Default listing
///    matches the "what still needs work?" mental model operators
///    reach for; the closed-ish statuses only come back via the
///    `show_all` escape hatch or an explicit `status` selector.
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
    // Every memory lives at `memories/<slug>/<uuid>.md`, so slug
    // leaf directories are the enumeration surface. FR-41-aware:
    // nested slug paths surface alongside flat ones.
    let slug_dirs = crate::memory::list_memory_slug_dirs(backend, &entry.handle, &Rev::head())
        .await
        .map_err(|e| FeatureError::Memory(ImportError::Git(e)))?;

    let mut out = Vec::new();
    for slug_dir in slug_dirs {
        match read_feature(backend, entry, &slug_dir.slug, None).await {
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

/// Body-free counterpart to [`list_features`] for listing surfaces
/// (MCP `list_features` tool, `mmcp feature list` CLI). Returns
/// per-FR metadata only; callers that need a body fetch the
/// individual record via [`read_feature`].
///
/// Filter precedence and sort order match [`list_features`]
/// exactly — this is a wire-shape change, not a semantics change.
///
/// Interim implementation reads full records and projects them
/// onto [`FeatureSummary`], discarding bodies. When the
/// frontmatter-only read primitive (`feature:frontmatter-only-read-
/// primitive-in-mmcp-store`) lands, this function swaps to it
/// without changing its signature.
pub async fn list_feature_summaries(
    backend: &NativeBackend,
    entry: &GroupEntry,
    status_filter: Option<FeatureStatus>,
    show_all: bool,
) -> Result<Vec<FeatureSummary>, FeatureError> {
    let records = list_features(backend, entry, status_filter, show_all).await?;
    Ok(records
        .into_iter()
        .map(FeatureSummary::from_record)
        .collect())
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

/// FR-44 entry point: resolve the project group using the explicit
/// selector when provided, falling back to the cwd walk otherwise.
///
/// `project` accepts a UUID or a slug and resolves against the
/// local mirror via [`crate::memory::resolve_group`]. Unknown
/// identifier surfaces [`FeatureError::UnknownProject`]. When
/// `project` is `None`, this is a bare `resolve_project_group`
/// call — the historical cwd walk stays the default so every
/// existing caller keeps working.
///
/// Returns `(entry, project_root_or_cwd)`. The second slot is
/// only meaningful for the cwd-walk branch (callers use it to
/// locate `.mmcp.toml`-adjacent files like `CLAUDE.md`); for the
/// explicit-selector branch we return `cwd` unchanged because the
/// selected project may not have a local filesystem root at all.
pub async fn resolve_project_group_with_selector(
    groups: &GroupIndex,
    project: Option<&str>,
    cwd: &Path,
) -> Result<(GroupEntry, PathBuf), FeatureError> {
    match project {
        Some(query) => {
            let entry = crate::memory::resolve_group(groups, query)
                .await
                .map_err(|_| FeatureError::UnknownProject {
                    query: query.to_string(),
                })?;
            Ok((entry, cwd.to_path_buf()))
        }
        None => resolve_project_group(groups, cwd).await,
    }
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
            depends_on: vec![prior_id],
            blocks: vec![later_id],
            ..AddSpec::default()
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

        // Cover the visibility matrix: two visible (Open +
        // Blocked), two default-hidden (Resolved + Duplicate).
        // Superseded is intentionally omitted from this fixture
        // because it only reaches the on-disk state via the
        // two-commit supersede flow, which is exercised
        // separately by `list_default_hides_superseded_fr`
        // below.
        for (slug, status) in [
            ("fr-a", FeatureStatus::Open),
            ("fr-b", FeatureStatus::Resolved),
            ("fr-c", FeatureStatus::Blocked),
            ("fr-d", FeatureStatus::Duplicate),
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

        // Explicit status selector wins over the default filter —
        // even with `show_all=false` the caller receives every FR
        // matching the requested status.
        let opens =
            list_feature_summaries(scratch.backend(), &entry, Some(FeatureStatus::Open), false)
                .await
                .expect("list open");
        let open_slugs: Vec<_> = opens.into_iter().map(|s| s.slug).collect();
        assert_eq!(open_slugs, vec!["fr-a".to_string()]);

        let resolved = list_feature_summaries(
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
            "explicit status filter wins over the default hide",
        );

        let duplicate = list_feature_summaries(
            scratch.backend(),
            &entry,
            Some(FeatureStatus::Duplicate),
            false,
        )
        .await
        .expect("list duplicate with show_all=false still returns matches");
        assert_eq!(duplicate.len(), 1);
    }

    #[tokio::test]
    async fn list_default_hides_resolved_and_duplicate_but_keeps_blocked() {
        // Default listing (status=None, show_all=false) hides only
        // the terminal-ish statuses: Resolved, Duplicate,
        // Superseded. In-progress-but-gated statuses (Blocked,
        // Deferred) stay visible so operators can still see what
        // is waiting on them. Open stays visible.
        let scratch = ScratchHome::new().await.expect("scratch home");
        let entry = seed_mixed_status_fixture(&scratch).await;

        let visible = list_feature_summaries(scratch.backend(), &entry, None, false)
            .await
            .expect("default list");
        let mut slugs: Vec<_> = visible.into_iter().map(|s| s.slug).collect();
        slugs.sort();
        assert_eq!(
            slugs,
            vec!["fr-a".to_string(), "fr-c".to_string()],
            "default listing must keep Open + Blocked and drop Resolved + Duplicate",
        );
    }

    #[tokio::test]
    async fn list_default_hides_superseded_fr() {
        // Supersede flow creates a Superseded FR via the canonical
        // two-commit path. Default listing drops it; show_all
        // surfaces both the old and the new.
        let scratch = ScratchHome::new().await.expect("scratch home");
        let seeded = scratch.seed_group("fr-group").await.expect("seed");
        let entry = scratch.groups().get(&seeded.group_id).await.expect("entry");

        add_feature(
            scratch.backend(),
            &entry,
            AddSpec {
                slug: Some("old".into()),
                title: "Old".into(),
                ..AddSpec::default()
            },
            scratch.author(),
        )
        .await
        .expect("seed old FR");

        add_feature(
            scratch.backend(),
            &entry,
            AddSpec {
                slug: Some("new".into()),
                title: "New".into(),
                supersedes: Some("old".into()),
                ..AddSpec::default()
            },
            scratch.author(),
        )
        .await
        .expect("add with supersedes");

        let visible = list_feature_summaries(scratch.backend(), &entry, None, false)
            .await
            .expect("default list");
        let slugs: Vec<_> = visible.into_iter().map(|s| s.slug).collect();
        assert_eq!(
            slugs,
            vec!["new".to_string()],
            "Superseded FR must drop out of the default listing",
        );

        let all = list_feature_summaries(scratch.backend(), &entry, None, true)
            .await
            .expect("show_all");
        assert_eq!(all.len(), 2, "show_all must re-include the superseded FR",);

        // Explicit status filter also surfaces it.
        let superseded_only = list_feature_summaries(
            scratch.backend(),
            &entry,
            Some(FeatureStatus::Superseded),
            false,
        )
        .await
        .expect("list superseded");
        assert_eq!(superseded_only.len(), 1);
        assert_eq!(superseded_only[0].slug, "old");
    }

    #[tokio::test]
    async fn list_show_all_returns_every_status() {
        // FR-024: `show_all=true` re-includes every FR regardless
        // of status. Pairs with the hide-by-default test above.
        let scratch = ScratchHome::new().await.expect("scratch home");
        let entry = seed_mixed_status_fixture(&scratch).await;

        let all = list_feature_summaries(scratch.backend(), &entry, None, true)
            .await
            .expect("list show_all");
        assert_eq!(
            all.len(),
            4,
            "show_all must re-include every FR regardless of status",
        );
    }

    #[tokio::test]
    async fn concurrent_add_feature_assigns_unique_numbers() {
        // FR-39: two concurrent `add_feature` calls with no
        // explicit number must not race on
        // `next_feature_number`. Before the per-group lock
        // landed, both calls read `max = N` at the same time and
        // both wrote `N + 1`, producing the duplicate-number
        // diagnose warning this test exists to prevent.
        let scratch = ScratchHome::new().await.expect("scratch home");
        let seeded = scratch.seed_group("fr-group").await.expect("seed");

        // Fire 8 concurrent add_feature tasks sharing the same
        // backend + group index. With the lock in place each
        // one acquires the per-group mutex serially and reads
        // the fresh max.
        let mut handles = Vec::new();
        for i in 0..8u32 {
            let backend = scratch.backend().clone();
            let groups = scratch.groups().clone();
            let gid = seeded.group_id;
            let author = scratch.author().clone();
            handles.push(tokio::spawn(async move {
                let entry = groups.get(&gid).await.expect("entry");
                add_feature(
                    &backend,
                    &entry,
                    AddSpec {
                        slug: Some(format!("fr-race-{i}")),
                        title: format!("Race {i}"),
                        ..AddSpec::default()
                    },
                    &author,
                )
                .await
                .expect("add")
            }));
        }
        let mut numbers: Vec<u32> = Vec::new();
        for h in handles {
            let rec = h.await.expect("task ok");
            numbers.push(rec.number.expect("auto-assigned number"));
        }
        numbers.sort_unstable();
        let unique_len = {
            let mut n = numbers.clone();
            n.dedup();
            n.len()
        };
        assert_eq!(
            unique_len,
            numbers.len(),
            "every concurrent add_feature must get a unique number, got {numbers:?}"
        );
        assert_eq!(
            numbers,
            (1..=8).collect::<Vec<_>>(),
            "8 concurrent creates with no seeds must produce 1..=8 monotonic",
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
    async fn rename_feature_rejects_oversized_message() {
        let scratch = ScratchHome::new().await.expect("scratch home");
        let seeded = scratch.seed_group("fr-group").await.expect("seed");
        let entry = scratch.groups().get(&seeded.group_id).await.expect("entry");

        add_feature(
            scratch.backend(),
            &entry,
            AddSpec {
                slug: Some("msg-src".into()),
                title: "Original".into(),
                body: "body".into(),
                ..AddSpec::default()
            },
            scratch.author(),
        )
        .await
        .expect("seed");

        let oversized = "a".repeat(mmcp_core::memory::MAX_MESSAGE_LENGTH + 1);
        let err = rename_feature(
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
            FeatureError::Memory(ImportError::FieldTooLong(_))
        ));
        // Rejected before any commit: the feature is still at its
        // original slug.
        read_feature(scratch.backend(), &entry, "msg-src", None)
            .await
            .expect("still at original slug");
    }

    #[tokio::test]
    async fn rename_feature_accepts_message_within_bound() {
        let scratch = ScratchHome::new().await.expect("scratch home");
        let seeded = scratch.seed_group("fr-group").await.expect("seed");
        let entry = scratch.groups().get(&seeded.group_id).await.expect("entry");

        add_feature(
            scratch.backend(),
            &entry,
            AddSpec {
                slug: Some("msg-src-ok".into()),
                title: "Original".into(),
                body: "body".into(),
                ..AddSpec::default()
            },
            scratch.author(),
        )
        .await
        .expect("seed");

        let bounded = "a".repeat(mmcp_core::memory::MAX_MESSAGE_LENGTH);
        let moved = rename_feature(
            scratch.backend(),
            &entry,
            "msg-src-ok",
            "msg-dst-ok",
            scratch.author(),
            Some(&bounded),
        )
        .await
        .expect("bounded message accepted");
        assert_eq!(moved[0].slug, "msg-dst-ok");
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
    async fn list_feature_summaries_drops_bodies_and_preserves_metadata() {
        let scratch = ScratchHome::new().await.expect("scratch home");
        let seeded = scratch.seed_group("fr-group").await.expect("seed");
        let entry = scratch.groups().get(&seeded.group_id).await.expect("entry");

        // Bodies large enough that any accidental inclusion in the
        // listing surface would be obvious in a wire-size assertion.
        let big_body = "x".repeat(8_192);
        for (slug, number) in [("alpha", 2), ("beta", 1)] {
            add_feature(
                scratch.backend(),
                &entry,
                AddSpec {
                    slug: Some(slug.into()),
                    title: slug.into(),
                    description: format!("desc-{slug}"),
                    body: big_body.clone(),
                    number: Some(number),
                    ..AddSpec::default()
                },
                scratch.author(),
            )
            .await
            .expect("seed");
        }

        let summaries = list_feature_summaries(scratch.backend(), &entry, None, true)
            .await
            .expect("list summaries");

        // Sort + status filter come from list_features and stay
        // unchanged: ascending by number, then slug.
        let slugs: Vec<_> = summaries.iter().map(|s| s.slug.as_str()).collect();
        assert_eq!(slugs, vec!["beta", "alpha"]);

        // Frontmatter-derived fields survive the projection.
        assert_eq!(summaries[0].title, "beta");
        assert_eq!(summaries[0].description, "desc-beta");
        assert_eq!(summaries[0].number, Some(1));
        assert_eq!(summaries[1].number, Some(2));

        // Compile-time guarantee: FeatureSummary has no `body`
        // field, so the wire shape can never regress to inlining
        // bodies. The runtime check is the size proxy below.
        let serialized_size: usize = summaries
            .iter()
            .map(|s| s.title.len() + s.description.len() + s.slug.len() + s.commit_id.len())
            .sum();
        assert!(
            serialized_size < big_body.len(),
            "summary payload must be much smaller than a single body",
        );
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

    fn forty_char_hex() -> &'static str {
        "0123456789abcdef0123456789abcdef01234567"
    }

    #[tokio::test]
    async fn add_feature_with_supersedes_marks_target_superseded() {
        // Two-commit supersede flow: the new FR is created and the
        // old FR gets `status = Superseded` plus a typed
        // `superseded_by` link pointing at the new FR's create
        // commit. The new FR's refs auto-includes the old one.
        let scratch = ScratchHome::new().await.expect("scratch home");
        let seeded = scratch.seed_group("fr-group").await.expect("seed");
        let entry = scratch.groups().get(&seeded.group_id).await.expect("entry");

        add_feature(
            scratch.backend(),
            &entry,
            AddSpec {
                slug: Some("old-fr".into()),
                title: "Old".into(),
                description: "original idea".into(),
                body: "body".into(),
                ..AddSpec::default()
            },
            scratch.author(),
        )
        .await
        .expect("seed old FR");

        let new_record = add_feature(
            scratch.backend(),
            &entry,
            AddSpec {
                slug: Some("new-fr".into()),
                title: "New".into(),
                description: "better-scoped replacement".into(),
                body: "body".into(),
                supersedes: Some("old-fr".into()),
                ..AddSpec::default()
            },
            scratch.author(),
        )
        .await
        .expect("add with supersedes");

        // Old FR now reports Superseded status with a typed back-link.
        let old_record = read_feature(scratch.backend(), &entry, "old-fr", None)
            .await
            .expect("read old after supersede");
        assert_eq!(old_record.status, FeatureStatus::Superseded);
        let link = old_record.superseded_by.expect("back-link set");
        assert_eq!(link.commit, new_record.commit_id);

        // New FR's refs auto-include the old FR's pre-supersede commit.
        let new_refs = read_memory_refs(
            scratch.backend(),
            &entry.handle,
            &resolve_memory(
                scratch.backend(),
                &entry.handle,
                Some(&new_record.slug),
                None,
            )
            .await
            .expect("resolve new")
            .path,
        )
        .await
        .expect("refs");
        assert_eq!(new_refs.len(), 1, "auto-ref must point at the old FR");
    }

    #[tokio::test]
    async fn add_feature_supersedes_unknown_slug_errors() {
        let scratch = ScratchHome::new().await.expect("scratch home");
        let seeded = scratch.seed_group("fr-group").await.expect("seed");
        let entry = scratch.groups().get(&seeded.group_id).await.expect("entry");

        let err = add_feature(
            scratch.backend(),
            &entry,
            AddSpec {
                slug: Some("new-fr".into()),
                title: "New".into(),
                supersedes: Some("does-not-exist".into()),
                ..AddSpec::default()
            },
            scratch.author(),
        )
        .await
        .expect_err("unknown supersede target must fail");
        match err {
            FeatureError::SupersedesUnknown { query } => {
                assert_eq!(query, "does-not-exist")
            }
            other => panic!("expected SupersedesUnknown, got {other:?}"),
        }
    }

    /// FR-51: superseding a Resolved FR captures the redesign-
    /// replaces-landed-design relationship as a typed
    /// `superseded_by` chain. The two-commit flow runs and the
    /// back-link symmetry holds — old FR flips to `Superseded`,
    /// new FR carries a ref pointing at the old FR's pre-supersede
    /// commit. Duplicate and Superseded targets stay rejected (see
    /// `add_feature_supersedes_already_superseded_fr_chains_the_link`).
    #[tokio::test]
    async fn add_feature_can_supersede_resolved_target() {
        let scratch = ScratchHome::new().await.expect("scratch home");
        let seeded = scratch.seed_group("fr-group").await.expect("seed");
        let entry = scratch.groups().get(&seeded.group_id).await.expect("entry");

        add_feature(
            scratch.backend(),
            &entry,
            AddSpec {
                slug: Some("old-resolved".into()),
                title: "Old".into(),
                status: FeatureStatus::Resolved,
                ..AddSpec::default()
            },
            scratch.author(),
        )
        .await
        .expect("seed resolved FR");

        let new_record = add_feature(
            scratch.backend(),
            &entry,
            AddSpec {
                slug: Some("new-fr".into()),
                title: "New".into(),
                supersedes: Some("old-resolved".into()),
                ..AddSpec::default()
            },
            scratch.author(),
        )
        .await
        .expect("supersede flow against Resolved target succeeds");

        let old_after = read_feature(scratch.backend(), &entry, "old-resolved", None)
            .await
            .expect("re-read old after supersede");
        assert_eq!(old_after.status, FeatureStatus::Superseded);
        let link = old_after.superseded_by.expect("back-link set");
        assert_eq!(link.commit, new_record.commit_id);

        let new_refs = read_memory_refs(
            scratch.backend(),
            &entry.handle,
            &resolve_memory(
                scratch.backend(),
                &entry.handle,
                Some(&new_record.slug),
                None,
            )
            .await
            .expect("resolve new")
            .path,
        )
        .await
        .expect("refs");
        assert_eq!(
            new_refs.len(),
            1,
            "new FR must carry exactly one auto-populated ref pinned at old FR's pre-supersede commit",
        );
    }

    #[tokio::test]
    async fn add_feature_supersedes_already_superseded_fr_chains_the_link() {
        // Superseding an already-superseded FR surfaces the
        // existing back-link in the error so callers can chase to
        // the tip of the chain.
        let scratch = ScratchHome::new().await.expect("scratch home");
        let seeded = scratch.seed_group("fr-group").await.expect("seed");
        let entry = scratch.groups().get(&seeded.group_id).await.expect("entry");

        add_feature(
            scratch.backend(),
            &entry,
            AddSpec {
                slug: Some("old-fr".into()),
                title: "Old".into(),
                ..AddSpec::default()
            },
            scratch.author(),
        )
        .await
        .expect("seed old FR");

        let middle = add_feature(
            scratch.backend(),
            &entry,
            AddSpec {
                slug: Some("middle-fr".into()),
                title: "Middle".into(),
                supersedes: Some("old-fr".into()),
                ..AddSpec::default()
            },
            scratch.author(),
        )
        .await
        .expect("first supersede");

        let err = add_feature(
            scratch.backend(),
            &entry,
            AddSpec {
                slug: Some("newest-fr".into()),
                title: "Newest".into(),
                supersedes: Some("old-fr".into()),
                ..AddSpec::default()
            },
            scratch.author(),
        )
        .await
        .expect_err("re-superseding an already-superseded FR must fail");
        match err {
            FeatureError::SupersedesInvalidStatus {
                status,
                existing_link,
                ..
            } => {
                assert_eq!(status, FeatureStatus::Superseded);
                let link = existing_link.expect("chain-pointer set");
                assert_eq!(link.commit, middle.commit_id);
            }
            other => panic!("expected SupersedesInvalidStatus, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn update_feature_refs_add_remove_compose_by_target() {
        // refs_remove strips by target UUID (commit sha ignored);
        // refs_add then dedupe-replaces by target so add-side
        // commit pins win.
        let scratch = ScratchHome::new().await.expect("scratch home");
        let seeded = scratch.seed_group("fr-group").await.expect("seed");
        let entry = scratch.groups().get(&seeded.group_id).await.expect("entry");

        let seed = add_feature(
            scratch.backend(),
            &entry,
            AddSpec {
                slug: Some("fr-refs".into()),
                title: "t".into(),
                ..AddSpec::default()
            },
            scratch.author(),
        )
        .await
        .expect("seed");

        // Seed two refs via update (add-side only).
        let keep = Uuid::now_v7();
        let drop = Uuid::now_v7();
        let ref_keep = MemoryRef::new(keep, forty_char_hex());
        let ref_drop = MemoryRef::new(drop, forty_char_hex());
        update_feature(
            scratch.backend(),
            &entry,
            &seed.slug,
            UpdateSpec {
                refs_add: Some(vec![ref_keep.clone(), ref_drop.clone()]),
                ..UpdateSpec::default()
            },
            scratch.author(),
        )
        .await
        .expect("seed refs");

        // Drop one, replace the kept one with a newer commit pin.
        let new_commit = "fedcba9876543210fedcba9876543210fedcba98";
        let ref_keep_updated = MemoryRef::new(keep, new_commit);
        update_feature(
            scratch.backend(),
            &entry,
            &seed.slug,
            UpdateSpec {
                refs_remove: Some(vec![drop]),
                refs_add: Some(vec![ref_keep_updated.clone()]),
                ..UpdateSpec::default()
            },
            scratch.author(),
        )
        .await
        .expect("compose refs");

        // Re-read raw refs and assert the shape.
        let resolved = resolve_memory(scratch.backend(), &entry.handle, Some(&seed.slug), None)
            .await
            .expect("resolve");
        let refs = read_memory_refs(scratch.backend(), &entry.handle, &resolved.path)
            .await
            .expect("refs");
        assert_eq!(refs.len(), 1, "drop + replace leaves exactly one ref");
        assert_eq!(refs[0].target, keep);
        assert_eq!(refs[0].commit, new_commit);
    }
}
