//! Memory CRUD primitives plus the `import_memory` upsert wrapper.
//!
//! Three typed primitives (`create_memory_file`, `update_memory_file`, `delete_memory_file`),
//! wrap `NativeBackend::write_commit` with existence checks,
//! so every consumer, the CLI, the MCP tools, the GUI, or any third-party caller,
//! enforces a strict contract without re-implementing the probe.
//! Each primitive maps a missing or collision slug to a structured [`ImportError`] variant.
//!
//! `import_memory` is the higher-level wrapper used by the CLI `mmcp import` path,
//! and by the MCP `write_memory` tool:
//! it parses or synthesises frontmatter, chooses between create and update,
//! based on the caller-supplied `override_existing` flag, and commits.
//!
//! `ImportError` keeps its name even though the module covers broader CRUD concerns;
//! a rename to `MemoryError` would ripple across every consumer's error mapper,
//! not worth it for this chain.

use std::future::Future;

use mmcp_core::id::{GroupId, MemoryId};
use mmcp_core::memory::{MemoryFile, MemoryFrontmatter, MemoryKind};
use mmcp_git::{CommitSpec, GitBackend, GitError, NativeBackend, RepoHandle, Rev};
use uuid::Uuid;

use crate::groups::{GroupEntry, GroupIndex};
use crate::home::ResolvedAuthor;

/// Batched-read outcome shape [`NativeBackend::read_files`] returns.
/// Spelled out locally because its own alias sits in a private module of `mmcp-git`.
type BatchOutcome = Vec<(String, Result<bytes::Bytes, GitError>)>;

/// Result of a successful import.
#[derive(Debug, Clone)]
pub struct ImportResult {
    pub slug: String,
    /// Canonical UUID minted for this memory (or taken from the source's frontmatter when it carried one).
    /// Callers use this to address the memory across the two-level `memories/<slug>/<uuid>.md` layout
    /// without re-resolving by slug, which is ambiguous once siblings exist.
    pub id: Uuid,
    pub commit_id: String,
}

/// Synthetic frontmatter for files that lack `+++` fences.
#[derive(Debug, Clone)]
pub struct SynthFrontmatter {
    pub name: String,
    pub description: String,
    pub kind: MemoryKind,
}

/// Errors specific to memory CRUD operations.
///
/// Create, update, delete, and import share this one error type.
#[derive(Debug, thiserror::Error)]
pub enum ImportError {
    #[error(
        "invalid slug '{0}': must be 1-{MAX_SLUG_SEGMENTS} `/`-joined segments (each lowercase alphanumeric with hyphens, no leading/trailing hyphens, no `..`), total length up to {MAX_SLUG_LENGTH} chars"
    )]
    InvalidSlug(String),

    #[error("content has no +++ frontmatter and no synthetic frontmatter provided")]
    MissingFrontmatter,

    #[error("frontmatter parse error: {0}")]
    Parse(#[from] mmcp_core::memory::MemoryParseError),

    #[error("git error: {0}")]
    Git(#[from] GitError),

    #[error("render error: {0}")]
    Render(String),

    #[error(transparent)]
    UnknownKind(#[from] mmcp_core::memory::MemoryKindParseError),

    #[error("group not found: {0}")]
    GroupNotFound(String),

    /// A `create` was attempted against a slug that is already on disk.
    /// The caller picks between `update` (edit-in-place) and `create` with an explicit override to replace.
    #[error("memory '{slug}' already exists in this group")]
    MemoryAlreadyExists { slug: String },

    /// An `update` or `delete` was attempted against a slug that has no file in the group.
    /// Distinct from `GroupNotFound`, which signals a missing group altogether.
    /// The lookup may have been keyed on either `slug`, `id`, or both,
    /// so both fields are optional; callers populate whichever addresses they actually tried.
    #[error("memory not found (slug={slug:?}, id={id:?})")]
    MemoryNotFound {
        slug: Option<String>,
        id: Option<Uuid>,
    },

    /// A slug-only lookup resolved to more than one memory under `memories/<slug>/`.
    /// The caller must re-query with an explicit `id` from the candidate list.
    #[error("memory slug '{slug}' has multiple entries; disambiguate with id")]
    MemoryAmbiguous { slug: String, candidates: Vec<Uuid> },

    /// Both `slug` and `id` were supplied but the on-disk memory's frontmatter carries a different id.
    /// Signals either a stale client cache or a corrupted frontmatter pair.
    #[error("memory '{slug}' id mismatch: expected {expected}, got {got}")]
    MemoryIdMismatch {
        slug: String,
        expected: Uuid,
        got: Uuid,
    },

    /// A write addressed by filename UUID (the caller specified the filename path explicitly,
    /// or resolved via the filename fast path) carried a frontmatter `id` that disagrees with the filename.
    /// Rejected by default; callers that genuinely intend to overwrite a drifted file
    /// pass `force = true` to flip the rejection into an accepted-with-note path.
    #[error(
        "filename-addressed write to '{path}' has id mismatch: filename {filename}, frontmatter {frontmatter} (pass force=true to override)"
    )]
    IdMismatchOnFilenameWrite {
        path: String,
        filename: Uuid,
        frontmatter: Uuid,
    },

    /// Neither `slug` nor `id` was provided to a resolver call that
    /// requires at least one addressing key.
    #[error("resolve_memory requires at least one of slug or id")]
    ResolveArgsMissing,

    /// A user-supplied string field (body, name, description, a tag, or an explicit commit-message override)
    /// exceeded its bounded maximum length.
    /// See `mmcp_core::memory` for the named constants
    /// (`MAX_BODY_LENGTH`, `MAX_NAME_LENGTH`, `MAX_DESCRIPTION_LENGTH`, `MAX_TAG_LENGTH`,
    /// `MAX_TAG_COUNT`, `MAX_MESSAGE_LENGTH`).
    #[error("field too long: {0}")]
    FieldTooLong(#[from] mmcp_core::memory::FieldLengthError),

    /// [`parse_creatable_kind`] was pointed at a tracked kind (`feature` / `issue` / `milestone`).
    /// Tracked kinds carry a structured metadata subtable (`[feature]` / `[issue]` / `[milestone]`)
    /// this plain memory-write path never populates;
    /// accepting one here would silently create a memory `diagnose` then flags as defective,
    /// and every tracked-kind routing entry point (`read_milestone` etc.) rejects as `not_a_*`.
    #[error(
        "kind '{kind}' is a tracked kind and cannot be created via memory create/edit; use the dedicated add_{kind} command instead"
    )]
    NotACreatableKind { kind: String },

    /// A memory blob read back from git was not valid UTF-8.
    /// Surfaced as a typed error instead of lossily substituting the replacement character,
    /// which would silently corrupt frontmatter/body content instead of reporting the truncation.
    #[error("memory file '{path}' is not valid UTF-8")]
    NotUtf8 {
        path: String,
        #[source]
        source: std::str::Utf8Error,
    },

    /// The per-group ticket counter ([`crate::tracker::next_ticket_number`]) reached [`u32::MAX`].
    /// Surfaced instead of silently wrapping to `0` and reissuing an already-allocated feature/issue number.
    #[error(
        "ticket counter overflow: every number up to {} is already allocated",
        u32::MAX
    )]
    TicketCounterOverflow,

    /// [`read_and_apply_body_ops`]'s edit batch failed against the body it targeted.
    /// Causes: a section or anchor not found, an out-of-range line, or a line-op content guard mismatch.
    /// Boxed: `MemoryEditError`'s content-guard variants carry several owned `Vec`s.
    /// This is the variant that would otherwise set every `ImportError`-wrapping error type's minimum size.
    #[error(transparent)]
    Edit(#[from] Box<crate::memory_ops::MemoryEditError>),
}

/// Per-file reference to a memory on disk.
/// Returned by [`list_all_memory_files`] so callers get a direct path
/// plus the slug/id pair the `memories/<slug>/<uuid>.md` layout encodes.
#[derive(Debug, Clone)]
pub struct MemoryFileRef {
    pub slug: String,
    pub id: Uuid,
    pub path: String,
}

/// One leaf slug directory found by [`list_memory_slug_dirs`].
/// "Leaf" means a tree node that holds at least one direct `.md` blob;
/// intermediate path nodes that only contain subtrees are not surfaced.
/// The `slug` field is the full slash-joined path from `memories/` down (e.g. `feedback/git/commit-phase`).
#[derive(Debug, Clone)]
pub struct MemorySlugDir {
    /// Path slug (one or more `/`-joined segments, no leading
    /// `memories/` prefix).
    pub slug: String,
    /// Full in-repo directory path: `memories/<slug>`.
    pub dir: String,
    /// Direct file entries returned by `list_tree(dir)`.
    /// Includes non-UUID names so diagnostics can flag schema violations without re-listing.
    pub filenames: Vec<String>,
}

/// Walk the `memories/` tree recursively and surface every leaf slug directory under it.
/// A directory counts as a leaf iff it holds at least one direct `.md` blob;
/// intermediate path nodes (only subtrees, no direct files) are traversed transparently.
/// A leaf may also have child slug paths of its own
/// (e.g. `feedback/<uuid>.md` and `feedback/git/<uuid>.md` can coexist);
/// [`NativeBackend::list_tree_recursive`] already reaches every directory regardless of
/// its own leaf status, so every nested leaf still surfaces here.
///
/// Slug paths may contain `/`-separated segments; this helper is the shared enumeration primitive,
/// every listing surface (memories, features, issues, tracker, diagnostics) routes through,
/// so a nested slug never goes invisible.
pub async fn list_memory_slug_dirs(
    backend: &NativeBackend,
    handle: &RepoHandle,
    rev: &Rev,
) -> Result<Vec<MemorySlugDir>, GitError> {
    let root = mmcp_core::conventions::MEMORIES_DIR;
    let ext = mmcp_core::conventions::MEMORY_EXTENSION;
    let dirs = backend.list_tree_recursive(handle, root, rev).await?;
    let mut out: Vec<MemorySlugDir> = dirs
        .into_iter()
        .filter(|(slug, filenames)| !slug.is_empty() && filenames.iter().any(|f| f.ends_with(ext)))
        .map(|(slug, filenames)| MemorySlugDir {
            dir: format!("{root}/{slug}"),
            slug,
            filenames,
        })
        .collect();
    out.sort_by(|a, b| a.slug.cmp(&b.slug));
    Ok(out)
}

/// Walk every memory file in the group at `rev`.
/// Every leaf slug directory under `memories/` is enumerated and every UUID-named `.md` file inside
/// surfaces as one entry; duplicate slugs appear as multiple entries with distinct UUIDs.
///
/// Used by diagnostics and any other consumer that needs to read every memory exactly once.
/// Nested slug paths surface alongside flat ones because the walk is recursive.
pub async fn list_all_memory_files(
    backend: &NativeBackend,
    handle: &RepoHandle,
    rev: &Rev,
) -> Result<Vec<MemoryFileRef>, GitError> {
    let ext = mmcp_core::conventions::MEMORY_EXTENSION;
    let mut out = Vec::new();
    for entry in list_memory_slug_dirs(backend, handle, rev).await? {
        for filename in &entry.filenames {
            let Some(stem) = filename.strip_suffix(ext) else {
                continue;
            };
            let Ok(id) = Uuid::parse_str(stem) else {
                // Ignore stray non-UUID files; the layout contract
                // says every memory filename is a UUIDv7.
                continue;
            };
            out.push(MemoryFileRef {
                slug: entry.slug.clone(),
                id,
                path: format!("{}/{filename}", entry.dir),
            });
        }
    }
    Ok(out)
}

/// Count every memory file [`list_all_memory_files`] would return, without
/// materializing a [`MemoryFileRef`] (slug clone, parsed id, formatted
/// path) for each one; used where only the total is needed (`list_groups`'
/// `memory_count`), so a group with many files pays for one recursive tree
/// walk and a `usize` filter/count, never `N` per-file `String` allocations.
pub async fn count_all_memory_files(
    backend: &NativeBackend,
    handle: &RepoHandle,
    rev: &Rev,
) -> Result<usize, GitError> {
    count_all_memory_files_over(|| list_memory_slug_dirs(backend, handle, rev)).await
}

/// [`count_all_memory_files`]'s walk, factored out behind a seam so a test
/// can inject a call-counting stub. `walk` is expected to behave like
/// [`list_memory_slug_dirs`]: production passes it directly. Proves this
/// makes exactly one recursive tree walk regardless of how many slug
/// directories the group holds, guarding against a regression that loops
/// [`crate::tracker::count_slug_entries`]'s single-slug, single-`list_tree`
/// shape once per slug instead of walking the whole tree in one call.
async fn count_all_memory_files_over<F, Fut>(walk: F) -> Result<usize, GitError>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<Vec<MemorySlugDir>, GitError>>,
{
    let ext = mmcp_core::conventions::MEMORY_EXTENSION;
    let slug_dirs = walk().await?;
    Ok(slug_dirs
        .iter()
        .flat_map(|dir| &dir.filenames)
        .filter(|filename| {
            filename
                .strip_suffix(ext)
                .is_some_and(|stem| Uuid::parse_str(stem).is_ok())
        })
        .count())
}

/// How a [`ResolvedMemory`] was reached.
/// Branches the write enforcement rule:
/// filename-addressed writes reject on id mismatch unless `force`,
/// frontmatter-addressed writes accept with a `malformed_frontmatter` warning note,
/// and slug-only queries skip the mismatch check because no id was provided to compare against.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum AddressingMode {
    /// Reached via the filename fast path:
    /// file at `memories/<slug>/<id>.md` exists AND its frontmatter id matches the queried id.
    /// Writes in this mode treat the filename UUID as authoritative,
    /// and surface mismatches as hard rejections (unless the caller passes `force: true`).
    ByFilename,
    /// Reached by scanning frontmatter ids across the group after the filename fast path missed.
    /// Either the file was hand-crafted with a non-UUID filename,
    /// or its filename UUID disagrees with the stored frontmatter id.
    /// Writes in this mode accept the edit and emit a `malformed_frontmatter` warning,
    /// so the drift stays visible.
    ByFrontmatter,
    /// Reached via slug-only resolution; no id was supplied, so there is no filename/frontmatter comparison to make.
    /// The default: the mismatch check is a no-op without an id to compare against,
    /// so it is the safe "off" sentinel for a `Default`-derived options struct.
    #[default]
    BySlugOnly,
}

/// Outcome of the filename-vs-frontmatter id check that [`validate_id_mismatch`] runs on every write.
/// Callers map this onto `id_mismatch_accepted`/`id_mismatch_forced` notes at the tool boundary.
///
/// `Match` is the silent common case.
/// The two mismatch variants distinguish acceptance paths:
/// `MismatchAccepted` rides on frontmatter-as-truth, `MismatchForced` rides on caller-asserted override,
/// of the filename addressing rule (via `force`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdValidation {
    /// Filename UUID and frontmatter id agree, or one of them was absent (no comparison possible).
    Match,
    /// Filename != frontmatter; the write was addressed by frontmatter/slug only,
    /// so frontmatter is source of truth and the write proceeds.
    /// Surface as an `id_mismatch_accepted` note.
    MismatchAccepted { filename: Uuid, frontmatter: Uuid },
    /// Filename != frontmatter; the write was addressed by filename UUID,
    /// and the caller passed `force = true` to override the rejection rule.
    /// Surface as an `id_mismatch_forced` note.
    MismatchForced { filename: Uuid, frontmatter: Uuid },
}

/// Compare the filename UUID encoded in `path` against the frontmatter `id` stamped in `rendered`.
/// Applies the enforcement rules above and returns the resulting [`IdValidation`].
///
/// Pure function (no I/O).
/// Callers commit only after the validation resolves to a non-error outcome;
/// the note emitted from the returned variant tags the response so consumers see the drift.
pub fn validate_id_mismatch(
    path: &str,
    rendered: &str,
    addressing_mode: AddressingMode,
    force: bool,
) -> Result<IdValidation, ImportError> {
    // Extract filename UUID from path stem `<uuid>.md`.
    // If the path doesn't end in a UUID stem (hand-crafted slugs), there is nothing to compare; treat as Match.
    let filename = filename_uuid_from_path(path);
    let frontmatter = parse_frontmatter_id(rendered.as_bytes());
    let (filename, frontmatter) = match (filename, frontmatter) {
        (Some(f), Some(g)) => (f, g),
        // Either side absent means no comparison applies.
        // Diagnose separately flags missing frontmatter ids;
        // the resolver already requires one for `ByFrontmatter` resolution.
        _ => return Ok(IdValidation::Match),
    };
    if filename == frontmatter {
        return Ok(IdValidation::Match);
    }
    match addressing_mode {
        AddressingMode::ByFilename if !force => Err(ImportError::IdMismatchOnFilenameWrite {
            path: path.to_string(),
            filename,
            frontmatter,
        }),
        AddressingMode::ByFilename => Ok(IdValidation::MismatchForced {
            filename,
            frontmatter,
        }),
        AddressingMode::ByFrontmatter | AddressingMode::BySlugOnly => {
            Ok(IdValidation::MismatchAccepted {
                filename,
                frontmatter,
            })
        }
    }
}

/// Extract the trailing `<uuid>.md` stem from a `memories/<slug>/<uuid>.md` path.
/// Returns `None` for hand-crafted filenames whose stem is not a UUID.
///
/// `pub(crate)`: also used by [`crate::archive::import`]'s batched import path
/// to classify a snapshotted existing memory's addressing mode without
/// re-deriving the filename/frontmatter comparison rule.
pub(crate) fn filename_uuid_from_path(path: &str) -> Option<Uuid> {
    let stem = path
        .rsplit('/')
        .next()
        .and_then(|name| name.strip_suffix(mmcp_core::conventions::MEMORY_EXTENSION))?;
    Uuid::parse_str(stem).ok()
}

/// Extract the `<slug>` segment from a `memories/<slug>/<uuid>.md` path,
/// (slug may itself contain `/`-joined sub-segments).
/// Returns `None` when `path` does not follow the two-level convention,
/// (e.g. `.mmcp.toml`, a hand-crafted debug write).
/// Shared by the cache write-trigger hook in [`write_file_at_path`] so it never re-derives the slug from scratch.
fn slug_from_memory_path(path: &str) -> Option<String> {
    let rest = path
        .strip_prefix(mmcp_core::conventions::MEMORIES_DIR)?
        .strip_prefix('/')?;
    let (slug, _filename) = rest.rsplit_once('/')?;
    Some(slug.to_string())
}

/// Addressing result from [`resolve_memory`].
/// Carries the slug, the canonical UUID, and the in-repo path (`memories/<slug>/<uuid>.md`)
/// that a subsequent `read_file` can consume verbatim.
#[derive(Debug, Clone)]
pub struct ResolvedMemory {
    pub slug: String,
    pub id: Uuid,
    pub path: String,
    /// How the resolver located this entry.
    /// Callers that write branch on this to decide whether a filename/frontmatter id mismatch
    /// is a hard reject or a soft warning.
    pub addressing_mode: AddressingMode,
}

/// Locate a memory by slug, id, or both.
///
/// - `slug + id`: address `memories/<slug>/<id>.md`; verify that
///   the frontmatter id matches the caller-supplied id.
///   Frontmatter disagreement surfaces [`ImportError::MemoryIdMismatch`];
///   missing file surfaces [`ImportError::MemoryNotFound`].
/// - `slug` only: walk `memories/<slug>/` and use the single entry
///   found; `0` yields [`ImportError::MemoryNotFound`], `≥2`
///   yields [`ImportError::MemoryAmbiguous`].
/// - `id` only: enumerate `memories/*/` and pick the slug
///   directory whose listing contains `<id>.md`.
/// - neither: [`ImportError::ResolveArgsMissing`].
///
/// The helper does not read the memory body; it only resolves the
/// logical address to a filesystem path so callers can proceed
/// with `read_file` / `write_commit` on a known key.
pub async fn resolve_memory(
    backend: &NativeBackend,
    handle: &RepoHandle,
    slug: Option<&str>,
    id: Option<Uuid>,
) -> Result<ResolvedMemory, ImportError> {
    match (slug, id) {
        (Some(s), Some(i)) => resolve_slug_and_id(backend, handle, s, i).await,
        (Some(s), None) => resolve_by_slug(backend, handle, s).await,
        (None, Some(i)) => resolve_by_id(backend, handle, i).await,
        (None, None) => Err(ImportError::ResolveArgsMissing),
    }
}

async fn resolve_slug_and_id(
    backend: &NativeBackend,
    handle: &RepoHandle,
    slug: &str,
    expected: Uuid,
) -> Result<ResolvedMemory, ImportError> {
    let path = mmcp_core::conventions::memory_path(slug, MemoryId::from_uuid(expected));
    match backend.read_file(handle, &path, &Rev::head()).await {
        Ok(bytes) => {
            verify_id_match(slug, &bytes, expected)?;
            Ok(ResolvedMemory {
                slug: slug.to_string(),
                id: expected,
                path,
                addressing_mode: AddressingMode::ByFilename,
            })
        }
        Err(GitError::PathNotFound(_)) => Err(ImportError::MemoryNotFound {
            slug: Some(slug.to_string()),
            id: Some(expected),
        }),
        Err(err) => Err(ImportError::Git(err)),
    }
}

async fn resolve_by_slug(
    backend: &NativeBackend,
    handle: &RepoHandle,
    slug: &str,
) -> Result<ResolvedMemory, ImportError> {
    // `list_tree` on `memories/<slug>` returns only the direct
    // UUID-named blobs; subtrees would be a schema violation.
    let dir = format!("{}/{}", mmcp_core::conventions::MEMORIES_DIR, slug);
    let entries = backend.list_tree(handle, &dir, &Rev::head()).await?;
    let uuids: Vec<Uuid> = entries
        .iter()
        .filter_map(|name| {
            name.strip_suffix(mmcp_core::conventions::MEMORY_EXTENSION)
                .and_then(|stem| Uuid::parse_str(stem).ok())
        })
        .collect();
    match uuids.len() {
        1 => {
            let only = uuids[0];
            Ok(ResolvedMemory {
                slug: slug.to_string(),
                id: only,
                path: mmcp_core::conventions::memory_path(slug, MemoryId::from_uuid(only)),
                addressing_mode: AddressingMode::BySlugOnly,
            })
        }
        n if n >= 2 => Err(ImportError::MemoryAmbiguous {
            slug: slug.to_string(),
            candidates: uuids,
        }),
        _ => Err(ImportError::MemoryNotFound {
            slug: Some(slug.to_string()),
            id: None,
        }),
    }
}

async fn resolve_by_id(
    backend: &NativeBackend,
    handle: &RepoHandle,
    expected: Uuid,
) -> Result<ResolvedMemory, ImportError> {
    // Walk every slug directory once, splitting files into the three buckets the fallback chain works through:
    //   1. UUID-named files whose stem == `expected`        (step 1 candidates)
    //   2. non-UUID-named files (hand-crafted slugs)         (step 2 candidates)
    //   3. UUID-named files whose stem != `expected`         (step 3 candidates)
    // Every file read goes through `parse_frontmatter_id` so the frontmatter id is the source of truth.
    let rev = Rev::head();
    let slug_dirs = list_memory_slug_dirs(backend, handle, &rev).await?;

    let filename_ext = mmcp_core::conventions::MEMORY_EXTENSION;
    let mut step1_candidates: Vec<(String, String)> = Vec::new();
    let mut step2_candidates: Vec<(String, String)> = Vec::new();
    let mut step3_candidates: Vec<(String, String)> = Vec::new();

    for entry in &slug_dirs {
        for name in &entry.filenames {
            let Some(stem) = name.strip_suffix(filename_ext) else {
                // Non-`.md` files are a schema violation that
                // `diagnose` already flags; the resolver ignores
                // them so a stray `.DS_Store` doesn't poison the
                // fallback scan.
                continue;
            };
            let path = format!("{}/{name}", entry.dir);
            match Uuid::parse_str(stem) {
                Ok(file_uuid) if file_uuid == expected => {
                    step1_candidates.push((entry.slug.clone(), path));
                }
                Ok(_) => {
                    step3_candidates.push((entry.slug.clone(), path));
                }
                Err(_) => {
                    step2_candidates.push((entry.slug.clone(), path));
                }
            }
        }
    }

    // Step 1: filename fast path.
    // Stem already matches `expected`; verify the frontmatter id agrees before declaring a hit.
    for (slug, path) in &step1_candidates {
        let Ok(bytes) = backend.read_file(handle, path, &rev).await else {
            continue;
        };
        if parse_frontmatter_id(&bytes) == Some(expected) {
            return Ok(ResolvedMemory {
                slug: slug.clone(),
                id: expected,
                path: path.clone(),
                addressing_mode: AddressingMode::ByFilename,
            });
        }
    }

    // Step 2: hand-crafted memories (non-UUID filenames).
    // Parse frontmatter and match on its id.
    // Reached only when step 1 missed because most repos have no non-UUID files.
    // One batched round trip over every candidate instead of one `read_file` per candidate:
    // step 3's candidate set is essentially the whole corpus, so a sequential scan here
    // would cost one git round trip per memory in the group on every step-1 miss.
    if let Some(hit) = scan_candidates_for_id(
        &step2_candidates,
        expected,
        AddressingMode::ByFrontmatter,
        |paths| backend.read_files(handle, paths, &rev),
    )
    .await?
    {
        return Ok(hit);
    }

    // Step 3: UUID-named files whose filename stem disagrees with `expected`.
    // Their frontmatter may still match the queried id, a drift the resolver honours,
    // (frontmatter is source of truth) while leaving the addressing mode as `ByFrontmatter`,
    // so writes route through the soft-warning branch.
    // Reached only when steps 1 and 2 both missed; same batching rationale as step 2.
    if let Some(hit) = scan_candidates_for_id(
        &step3_candidates,
        expected,
        AddressingMode::ByFrontmatter,
        |paths| backend.read_files(handle, paths, &rev),
    )
    .await?
    {
        return Ok(hit);
    }

    Err(ImportError::MemoryNotFound {
        slug: None,
        id: Some(expected),
    })
}

/// Scan `candidates` for the entry whose frontmatter id equals `expected`, via one batched read.
///
/// `read_batch` is the batched-read seam: production passes [`NativeBackend::read_files`] directly.
/// A test can pass a call-counting stub instead, to prove this issues exactly one batch call
/// regardless of how many candidates it carries.
/// A per-candidate read or parse failure is skipped, never aborts the scan;
/// a hard failure of `read_batch` itself (the whole batch call) is the only error this raises.
/// Returns the first candidate (in `candidates` order) whose frontmatter id matches, or `None` on a full miss.
async fn scan_candidates_for_id<F, Fut>(
    candidates: &[(String, String)],
    expected: Uuid,
    addressing_mode: AddressingMode,
    read_batch: F,
) -> Result<Option<ResolvedMemory>, ImportError>
where
    F: FnOnce(Vec<String>) -> Fut,
    Fut: Future<Output = Result<BatchOutcome, GitError>>,
{
    if candidates.is_empty() {
        return Ok(None);
    }
    let paths: Vec<String> = candidates
        .iter()
        .map(|(_slug, path)| path.clone())
        .collect();
    // `read_files` resolves the commit and root tree once and returns
    // outcomes in request order, so zipping back onto `candidates` is safe.
    let batch = read_batch(paths).await?;
    for ((slug, path), (_batch_path, outcome)) in candidates.iter().zip(batch) {
        let Ok(bytes) = outcome else { continue };
        if parse_frontmatter_id(&bytes) == Some(expected) {
            return Ok(Some(ResolvedMemory {
                slug: slug.clone(),
                id: expected,
                path: path.clone(),
                addressing_mode,
            }));
        }
    }
    Ok(None)
}

/// Result entry from [`read_frontmatters_in_group`].
/// Carries the file reference (slug, id, repo path) alongside the frontmatter parse outcome,
/// so a single corrupt file does not abort the whole listing:
/// the caller can choose to ignore, log, or propagate per-entry errors.
#[derive(Debug)]
pub struct MemoryFrontmatterEntry {
    pub file: MemoryFileRef,
    pub frontmatter: Result<MemoryFrontmatter, ImportError>,
}

/// Read just the frontmatter of a memory by slug and/or id.
///
/// Resolves the memory via [`resolve_memory`] and parses the on-disk file via [`MemoryFile::parse`],
/// discarding the body before return.
/// Use this when the caller only needs frontmatter fields (kind, name, description, tags, mandatory),
/// and has many memories to scan,
/// e.g. a GUI memory-list panel rendering kind prefixes for every slug,
/// or a feature summariser collapsing records onto a metadata-only wire shape.
/// Single reads where the body is also needed should keep using the full read path.
///
/// Today this still allocates the body internally (parses through
/// `MemoryFile::parse` and drops the result); a future fence-slice
/// optimisation can land separately to skip the body String entirely
/// without changing the public signature.
pub async fn read_frontmatter(
    backend: &NativeBackend,
    handle: &RepoHandle,
    slug: Option<&str>,
    id: Option<Uuid>,
) -> Result<(ResolvedMemory, MemoryFrontmatter), ImportError> {
    let resolved = resolve_memory(backend, handle, slug, id).await?;
    let frontmatter = read_frontmatter_at(backend, handle, &Rev::head(), &resolved.path).await?;
    Ok((resolved, frontmatter))
}

/// Read every memory's frontmatter in a group at `rev`.
///
/// Wraps [`list_all_memory_files`] + per-file frontmatter parse into one fan-out call.
/// Per-file errors land inside each entry's `frontmatter` field rather than aborting the iteration,
/// so one malformed file in a 100-memory group does not blank the whole listing.
/// Top-level git errors (the directory walk itself) still surface as `Err`.
pub async fn read_frontmatters_in_group(
    backend: &NativeBackend,
    handle: &RepoHandle,
    rev: &Rev,
) -> Result<Vec<MemoryFrontmatterEntry>, GitError> {
    let files = list_all_memory_files(backend, handle, rev).await?;
    let mut out = Vec::with_capacity(files.len());
    for file in files {
        let frontmatter = read_frontmatter_at(backend, handle, rev, &file.path).await;
        out.push(MemoryFrontmatterEntry { file, frontmatter });
    }
    Ok(out)
}

/// Decode `bytes` as UTF-8 and parse them as a [`MemoryFile`].
///
/// Borrows `bytes` as UTF-8 instead of lossily substituting the replacement character.
/// A malformed blob surfaces as [`ImportError::NotUtf8`], carrying `path`.
/// This avoids silently corrupting frontmatter or body content.
pub fn parse_memory_file_bytes(bytes: &[u8], path: &str) -> Result<MemoryFile, ImportError> {
    let text = std::str::from_utf8(bytes).map_err(|source| ImportError::NotUtf8 {
        path: path.to_string(),
        source,
    })?;
    Ok(MemoryFile::parse(text)?)
}

pub(crate) async fn read_frontmatter_at(
    backend: &NativeBackend,
    handle: &RepoHandle,
    rev: &Rev,
    path: &str,
) -> Result<MemoryFrontmatter, ImportError> {
    let bytes = backend.read_file(handle, path, rev).await?;
    let file = parse_memory_file_bytes(&bytes, path)?;
    Ok(file.frontmatter)
}

fn verify_id_match(slug: &str, bytes: &[u8], expected: Uuid) -> Result<(), ImportError> {
    let Some(actual) = parse_frontmatter_id(bytes) else {
        return Err(ImportError::MemoryNotFound {
            slug: Some(slug.to_string()),
            id: Some(expected),
        });
    };
    if actual == expected {
        Ok(())
    } else {
        Err(ImportError::MemoryIdMismatch {
            slug: slug.to_string(),
            expected,
            got: actual,
        })
    }
}

fn parse_frontmatter_id(bytes: &[u8]) -> Option<Uuid> {
    let text = std::str::from_utf8(bytes).ok()?;
    let file = MemoryFile::parse(text).ok()?;
    file.frontmatter.id
}

/// Optional config for [`write_file_at_path`].
/// The mandatory inputs (`backend`, `handle`, `path`, `rendered`, `author`) stay positional;
/// this bundles the addressing/override knobs most callers thread straight through from a resolver or CLI args.
///
/// `addressing_mode` and `force` drive the id-mismatch check;
/// `message` overrides the auto-generated commit message.
/// Construct with field-init shorthand plus `..Default::default()`,
/// so adding a field later is non-breaking for callers taking the default.
#[derive(Debug, Clone, Copy, Default)]
pub struct WriteFileOptions<'a> {
    pub addressing_mode: AddressingMode,
    pub force: bool,
    pub message: Option<&'a str>,
}

/// Enforce the bounded-length invariants on the rendered content of every write
/// that reaches [`write_file_at_path`]: parse `rendered` and check its frontmatter
/// (`name`, `description`, `tags`) and body against `mmcp_core::memory`'s named maxima.
///
/// This is the single choke point every memory write with rendered content commits through,
/// create, update, `edit_memory_body`, feature/issue create and update (see [`write_file_at_path`]
/// and [`write_memory_by_id`]), plus [`crate::archive::import`]'s batched multi-memory commit,
/// which calls this directly per accumulated file since it builds one [`CommitSpec`] for the
/// whole group instead of routing each memory through [`write_file_at_path`].
/// The check still runs exactly once per write regardless of which entry point triggered it,
/// per the SSOT/DRY rule and the "validation runs at the boundary" clause of `global-security-rules`.
/// Commit-message validation is a separate concern handled uniformly by [`resolve_commit_message`],
/// which every commit-producing entry point in this crate (including the ones with no rendered content,
/// like delete and move) calls instead of building its message inline.
pub(crate) fn validate_write_content_lengths(rendered: &str) -> Result<(), ImportError> {
    let file = MemoryFile::parse(rendered)?;
    mmcp_core::memory::validate_frontmatter_lengths(&file.frontmatter)?;
    mmcp_core::memory::validate_body_length(&file.body)?;
    Ok(())
}

/// Resolve the commit message for a write in this crate: validate
/// an explicit caller-supplied override against
/// [`mmcp_core::memory::validate_message_length`], or synthesize
/// one from `fallback` when the caller supplied none.
///
/// The single choke point every commit-producing entry point in
/// this crate resolves its message through before calling
/// `NativeBackend::write_commit`: [`write_file_at_path`],
/// [`delete_file_at_path`], [`move_memory_path`],
/// `features::rename_feature`, and `issues::rename_issue` all call
/// this instead of building their message inline, so a
/// caller-supplied override can never reach git unbounded
/// regardless of which entry point produced it.
pub fn resolve_commit_message(
    message: Option<&str>,
    fallback: impl FnOnce() -> String,
) -> Result<String, ImportError> {
    match message {
        Some(msg) => {
            mmcp_core::memory::validate_message_length(msg)?;
            Ok(msg.to_string())
        }
        None => Ok(fallback()),
    }
}

/// Read the memory file at `path`, apply `ops` to its body, and render the result.
///
/// Callers must hold the memory's exclusive lock (`crate::lock::memory_chain`) before calling.
/// The read, the ops' application, and the render all run against one snapshot.
/// A lock acquired only around the later write leaves the whole application window open to a concurrent writer.
/// Such a writer could shift the very lines the ops target.
/// Decodes strictly via [`parse_memory_file_bytes`], never lossily.
pub async fn read_and_apply_body_ops(
    backend: &NativeBackend,
    handle: &RepoHandle,
    path: &str,
    ops: &[crate::memory_ops::MemoryEditOp],
) -> Result<(MemoryFile, String), ImportError> {
    let bytes = backend.read_file(handle, path, &Rev::head()).await?;
    let mut file = parse_memory_file_bytes(&bytes, path)?;
    file.body = crate::memory_ops::apply_ops(&file.body, ops).map_err(Box::new)?;
    let rendered = file.to_string()?;
    Ok((file, rendered))
}

/// Commit a write of `rendered` at an explicit repo-relative `path` after running the id-mismatch check.
/// The validation compares the filename UUID encoded in `path` to the frontmatter `id` in `rendered`,
/// and applies the rules from [`validate_id_mismatch`].
/// Callers thread the `addressing_mode` from their resolver and `force` from their tool args,
/// via [`WriteFileOptions`].
///
/// On success returns the commit id and the [`IdValidation`] outcome,
/// so the caller can surface `id_mismatch_*` notes.
/// `IdMismatchOnFilenameWrite` short-circuits before writing.
pub async fn write_file_at_path(
    backend: &NativeBackend,
    handle: &RepoHandle,
    path: &str,
    rendered: &str,
    author: &ResolvedAuthor,
    options: WriteFileOptions<'_>,
) -> Result<(String, IdValidation), ImportError> {
    let WriteFileOptions {
        addressing_mode,
        force,
        message,
    } = options;
    validate_write_content_lengths(rendered)?;
    let validation = validate_id_mismatch(path, rendered, addressing_mode, force)?;
    let commit_message = resolve_commit_message(message, || format!("write {path}"))?;
    let commit_id = backend
        .write_commit(
            handle,
            CommitSpec::mmcp_commit(
                commit_message,
                vec![(path.to_string(), Some(rendered.as_bytes().to_vec()))],
                &author.name,
                &author.email,
            ),
        )
        .await?;

    // Write-trigger for the local content cache (see
    // `crate::cache`): best-effort, never fails the write itself.
    // The frontmatter id is authoritative when present (matches
    // what `validate_id_mismatch` above already treated as source
    // of truth for a frontmatter/filename disagreement); the
    // filename UUID is the fallback for content that somehow lacks
    // one.
    if let Some(slug) = slug_from_memory_path(path) {
        let id =
            parse_frontmatter_id(rendered.as_bytes()).or_else(|| filename_uuid_from_path(path));
        if let Some(id) = id {
            crate::cache::notify_write(handle.group_id, id, &slug, path, &commit_id, rendered)
                .await;
        }
    }

    Ok((commit_id, validation))
}

/// Commit a deletion of `path`.
/// Unconditional; callers probe first if they want a "not found" error.
pub async fn delete_file_at_path(
    backend: &NativeBackend,
    handle: &RepoHandle,
    path: &str,
    author: &ResolvedAuthor,
    message: Option<&str>,
) -> Result<String, ImportError> {
    let commit_message = resolve_commit_message(message, || format!("delete {path}"))?;
    let commit_id = backend
        .write_commit(
            handle,
            CommitSpec::mmcp_commit(
                commit_message,
                vec![(path.to_string(), None)],
                &author.name,
                &author.email,
            ),
        )
        .await?;
    Ok(commit_id)
}

/// Outcome of a successful [`move_memory_path`] call.
/// The id and body bytes are unchanged: the move is purely a slug/path rewrite.
#[derive(Debug, Clone)]
pub struct MoveMemoryOutcome {
    pub old_slug: String,
    pub new_slug: String,
    pub id: Uuid,
    pub old_path: String,
    pub new_path: String,
    pub commit_id: String,
}

/// Atomically rename a memory's slug path inside its group.
/// Single commit: writes the bytes at the new path and removes the file at the old path in the same tree rewrite,
/// so `git log` never shows a half-moved state.
/// The frontmatter (id, name, body, ...) is preserved verbatim so cross-refs stay valid.
///
/// Validation: both slugs must pass [`validate_memory_slug`].
/// The source memory is resolved via [`resolve_memory`] so callers may address it by `slug + id`, slug only, or id only.
/// Refuses to overwrite an existing memory at `new_slug` with the same id.
/// An explicit `message` override is bounded via [`resolve_commit_message`],
/// same as every other commit-producing entry point in this crate.
///
/// Same-slug moves short-circuit and return without committing: the operation is a no-op.
pub async fn move_memory_path(
    backend: &NativeBackend,
    handle: &RepoHandle,
    old_slug: Option<&str>,
    id: Option<Uuid>,
    new_slug: &str,
    author: &ResolvedAuthor,
    message: Option<&str>,
) -> Result<MoveMemoryOutcome, ImportError> {
    validate_memory_slug(new_slug)?;
    let resolved = resolve_memory(backend, handle, old_slug, id).await?;
    if resolved.slug == new_slug {
        // No-op: the move target is the source.
        // Returning a fake commit id would mislead callers;
        // surface the unchanged path so they know the memory already lives where they asked.
        return Ok(MoveMemoryOutcome {
            old_slug: resolved.slug.clone(),
            new_slug: resolved.slug,
            id: resolved.id,
            old_path: resolved.path.clone(),
            new_path: resolved.path,
            commit_id: String::new(),
        });
    }
    // Refuse to overwrite a sibling at the destination with the same id.
    // Writing different bytes there silently would lose data;
    // the caller should pick a different target or delete the existing entry first.
    let new_path = mmcp_core::conventions::memory_path(new_slug, MemoryId::from_uuid(resolved.id));
    match backend.read_file(handle, &new_path, &Rev::head()).await {
        Ok(_) => {
            return Err(ImportError::MemoryAlreadyExists {
                slug: new_slug.to_string(),
            });
        }
        Err(GitError::PathNotFound(_)) => {}
        Err(other) => return Err(ImportError::Git(other)),
    }
    let bytes = backend
        .read_file(handle, &resolved.path, &Rev::head())
        .await?;
    let commit_message = resolve_commit_message(message, || {
        format!(
            "move memory {} -> {} ({})",
            resolved.slug, new_slug, resolved.id
        )
    })?;
    let commit_id = backend
        .write_commit(
            handle,
            CommitSpec::mmcp_commit(
                commit_message,
                vec![
                    (new_path.clone(), Some(bytes.to_vec())),
                    (resolved.path.clone(), None),
                ],
                &author.name,
                &author.email,
            ),
        )
        .await?;
    Ok(MoveMemoryOutcome {
        old_slug: resolved.slug,
        new_slug: new_slug.to_string(),
        id: resolved.id,
        old_path: resolved.path,
        new_path,
        commit_id,
    })
}

/// Optional config for [`write_memory_by_id`].
/// Wraps the [`WriteFileOptions`] tail (`addressing_mode`, `force`, `message`)
/// delegated to [`write_file_at_path`] plus the create-or-override toggle this entry point owns.
/// Same field-init-shorthand-plus-`..Default::default()` construction pattern.
#[derive(Debug, Clone, Copy, Default)]
pub struct WriteMemoryOptions<'a> {
    pub override_existing: bool,
    pub addressing_mode: AddressingMode,
    pub force: bool,
    pub message: Option<&'a str>,
}

/// Write a memory at the two-level `memories/<slug>/<id>.md` path with create-or-override semantics.
/// Delegates the id-mismatch check to [`write_file_at_path`],
/// so callers thread `addressing_mode` and `force` through both primitives via [`WriteMemoryOptions`].
///
/// Returns [`ImportError::MemoryAlreadyExists`] on collision when
/// `override_existing` is `false`; otherwise overwrites in place.
pub async fn write_memory_by_id(
    backend: &NativeBackend,
    handle: &RepoHandle,
    slug: &str,
    id: Uuid,
    rendered: &str,
    author: &ResolvedAuthor,
    options: WriteMemoryOptions<'_>,
) -> Result<(String, IdValidation), ImportError> {
    let WriteMemoryOptions {
        override_existing,
        addressing_mode,
        force,
        message,
    } = options;
    validate_memory_slug(slug)?;
    let path = mmcp_core::conventions::memory_path(slug, MemoryId::from_uuid(id));
    let exists = match backend.read_file(handle, &path, &Rev::head()).await {
        Ok(_) => true,
        Err(GitError::PathNotFound(_)) => false,
        Err(err) => return Err(ImportError::Git(err)),
    };
    if exists && !override_existing {
        return Err(ImportError::MemoryAlreadyExists {
            slug: slug.to_string(),
        });
    }
    let commit_message = message.map(str::to_string).unwrap_or_else(|| {
        if exists {
            format!("update memory {slug}/{id}")
        } else {
            format!("create memory {slug}/{id}")
        }
    });
    write_file_at_path(
        backend,
        handle,
        &path,
        rendered,
        author,
        WriteFileOptions {
            addressing_mode,
            force,
            message: Some(&commit_message),
        },
    )
    .await
}

/// Import a memory into a group repo.
///
/// Parses `content` as a full memory file (frontmatter + body);
/// when the content lacks a `+++` block, `synth_frontmatter` must supply name/description/kind,
/// and the rest of the body is treated as the payload.
/// The memory lands at `memories/<slug>/<uuid>.md`:
/// the id is taken from frontmatter when present, otherwise a fresh UUIDv7 is minted.
///
/// `override_existing` only matters when an explicit id from frontmatter collides with an existing file.
/// With a freshly minted id, the write always creates a new sibling under the slug directory
/// (duplicate slugs are legal).
pub async fn import_memory(
    backend: &NativeBackend,
    handle: &RepoHandle,
    slug: &str,
    content: &str,
    synth_frontmatter: Option<SynthFrontmatter>,
    author: &ResolvedAuthor,
    override_existing: bool,
) -> Result<ImportResult, ImportError> {
    validate_memory_slug(slug)?;

    let mut memory_file = if content.trim_start().starts_with("+++") {
        MemoryFile::parse(content)?
    } else if let Some(synth) = synth_frontmatter {
        MemoryFile {
            frontmatter: MemoryFrontmatter::new(synth.name, synth.description, synth.kind),
            body: content.to_string(),
            format: mmcp_core::memory::FrontmatterFormat::TomlPlus,
        }
    } else {
        return Err(ImportError::MissingFrontmatter);
    };

    let id = memory_file.frontmatter.id.unwrap_or_else(Uuid::now_v7);
    memory_file.frontmatter = memory_file.frontmatter.clone().with_id(id);
    let rendered = memory_file
        .to_string()
        .map_err(|e| ImportError::Render(e.to_string()))?;

    // `import_memory` creates (or replaces under override) one memory under `memories/<slug>/<id>.md`.
    // Takes the create chain (Process-Shared + Group-Exclusive),
    // so concurrent imports against the same group serialise on UUID minting and file creation regardless of kind.
    // The shared ticket counter and slug-uniqueness invariant both rely on the group-wide exclusive view.
    let _guards = crate::lock::acquire_chain(&crate::lock::create_chain(handle.group_id)).await;

    let message = format!("import memory {slug}/{id}");
    // `import_memory` mints `id` and stamps it into frontmatter on the line above,
    // so filename and frontmatter agree by construction.
    // Uses `BySlugOnly` (the import flow has no caller-supplied addressing) and `force=false`;
    // the mismatch check is a no-op here.
    let (commit_id, _validation) = write_memory_by_id(
        backend,
        handle,
        slug,
        id,
        &rendered,
        author,
        WriteMemoryOptions {
            override_existing,
            addressing_mode: AddressingMode::BySlugOnly,
            message: Some(&message),
            ..Default::default()
        },
    )
    .await?;

    Ok(ImportResult {
        slug: slug.to_string(),
        id,
        commit_id,
    })
}

/// Maximum number of `/`-separated segments in a memory slug path.
/// Bounded so a malicious or buggy caller cannot blow the directory tree out arbitrarily.
pub const MAX_SLUG_SEGMENTS: usize = 8;

/// Maximum total slug length, including separators.
/// 256 is well above the legitimate need (8 segments × 30 chars + 7 separators = 247) while still bounded.
pub const MAX_SLUG_LENGTH: usize = 256;

/// Validate a single slug segment (no `/`).
/// Used both for memory slug components and for top-level identifiers like group slugs
/// where path separators are never legal.
pub fn validate_slug_segment(segment: &str) -> Result<(), ImportError> {
    if segment.is_empty() {
        return Err(ImportError::InvalidSlug(segment.to_string()));
    }
    if segment.starts_with('-') || segment.ends_with('-') {
        return Err(ImportError::InvalidSlug(segment.to_string()));
    }
    for ch in segment.chars() {
        if !ch.is_ascii_lowercase() && !ch.is_ascii_digit() && ch != '-' {
            return Err(ImportError::InvalidSlug(segment.to_string()));
        }
    }
    if segment.contains("--") {
        return Err(ImportError::InvalidSlug(segment.to_string()));
    }
    Ok(())
}

/// Validate a memory slug path.
/// A slug is one to [`MAX_SLUG_SEGMENTS`] `/`-joined segments;
/// each segment matches the single-segment rules in [`validate_slug_segment`].
/// Total length is capped at [`MAX_SLUG_LENGTH`].
/// Existing flat slugs are just zero-`/` paths and stay valid.
pub fn validate_memory_slug(slug: &str) -> Result<(), ImportError> {
    if slug.is_empty() || slug.len() > MAX_SLUG_LENGTH {
        return Err(ImportError::InvalidSlug(slug.to_string()));
    }
    if slug.starts_with('/') || slug.ends_with('/') {
        return Err(ImportError::InvalidSlug(slug.to_string()));
    }
    let segments: Vec<&str> = slug.split('/').collect();
    if segments.is_empty() || segments.len() > MAX_SLUG_SEGMENTS {
        return Err(ImportError::InvalidSlug(slug.to_string()));
    }
    for segment in &segments {
        if *segment == ".." || *segment == "." {
            return Err(ImportError::InvalidSlug(slug.to_string()));
        }
        validate_slug_segment(segment).map_err(|_| ImportError::InvalidSlug(slug.to_string()))?;
    }
    Ok(())
}

/// Compiled-in fallback for [`slugify_filename`]'s auto-slug length cap when deriving a slug from a title.
/// Applies whenever the caller supplies no explicit slug.
/// This is a production default for auto-generation, not an acceptance ceiling.
/// [`MAX_SLUG_LENGTH`] still governs what `validate_memory_slug` *accepts*.
///
/// Value fixed at 64, not independently derived from a specific reference system.
/// The LOWEST-precedence tier of `resolve_max_auto_slug_length`.
/// Overridable per-call, per-machine (env var), or per-user (config file).
/// See that function's doc comment for the full cascade.
/// Duplicate slugs are legal regardless of which tier wins.
/// Memories address by slug+UUID, not slug alone.
/// Two long titles can collide on the same truncated slug.
/// That is not a correctness problem needing a disambiguating suffix.
pub const DEFAULT_MAX_AUTO_SLUG_LENGTH: usize = 64;

/// Environment variable that overrides the auto-slug length cap for every invocation on this machine,
/// second in precedence behind an explicit per-call override.
/// Mirrors the `MMCP_HOME` naming convention (see [`crate::home`]).
pub const MAX_AUTO_SLUG_LENGTH_ENV: &str = "MMCP_MAX_AUTO_SLUG_LENGTH";

/// Derive a slug from a filename.
///
/// Strips known import extensions (`.md`, plus [`crate::import_adoc::ADOC_EXTENSIONS`]) case-insensitively.
/// Delegates the rest to the `slug` crate.
/// The extra adoc / asciidoc cases exist so an operator importing `coding-rules.adoc` gets the `coding-rules` slug.
/// Otherwise the slug would be `coding-rules-adoc`.
/// The on-disk memory still lands as `.md`.
///
/// The result is then capped via `truncate_slug_at_hyphen_boundary`.
/// The cap comes from `resolve_max_auto_slug_length` with no per-call override: env var, user config, then default.
/// This keeps an auto-derived slug from a long title a sane, readable identifier.
/// It does not grow unboundedly with the title.
/// Callers that need a one-off cap call [`slugify_filename_with_cap`] directly.
pub fn slugify_filename(filename: &str) -> String {
    slugify_filename_with_cap(filename, None)
}

/// Same as [`slugify_filename`], with one difference.
/// `override_max_len`, when `Some` and non-zero, takes precedence over every other tier.
/// `None` or `Some(0)` falls through to the env var, then user config, then the compiled-in default.
pub fn slugify_filename_with_cap(filename: &str, override_max_len: Option<usize>) -> String {
    let stem = strip_known_import_extension(filename);
    let slug = slug::slugify(stem);
    let max_len = resolve_max_auto_slug_length(override_max_len);
    truncate_slug_at_hyphen_boundary(&slug, max_len)
}

/// Resolve the effective auto-slug length cap.
/// Highest-precedence source wins:
/// 1. `override_len`, an explicit per-call argument.
/// 2. [`MAX_AUTO_SLUG_LENGTH_ENV`] environment variable.
/// 3. `~/.mmcp/config.toml` `[limits] max_auto_slug_length`
///    ([`mmcp_core::config::UserConfig`]).
/// 4. [`DEFAULT_MAX_AUTO_SLUG_LENGTH`], the compiled-in fallback.
///
/// A zero or unparsable value at any tier is treated as absent and falls through to the next tier,
/// logged rather than silently discarded: a broken override must never make slug generation itself fail.
/// The tier-selection logic itself lives in [`resolve_from_tiers`], kept separate from the I/O
/// (env var read, config file read) so its precedence rules are unit-testable
/// without mutating process-global environment state.
fn resolve_max_auto_slug_length(override_len: Option<usize>) -> usize {
    let env_raw = std::env::var(MAX_AUTO_SLUG_LENGTH_ENV).ok();
    let env_len = parse_env_auto_slug_length(env_raw.as_deref());
    let config_len = user_config_max_auto_slug_length();
    resolve_from_tiers(override_len, env_len, config_len)
}

/// Parse the raw [`MAX_AUTO_SLUG_LENGTH_ENV`] value, if any, into a tier value.
/// Logs and falls through (returns `None`) when the variable is present but not a valid number,
/// rather than silently discarding it.
/// Split out from [`resolve_max_auto_slug_length`]
/// so this parse behavior is unit-testable without mutating process-global environment state.
fn parse_env_auto_slug_length(raw: Option<&str>) -> Option<usize> {
    let raw = raw?;
    match raw.parse::<usize>() {
        Ok(n) => Some(n),
        Err(err) => {
            tracing::warn!(
                env_value = %raw,
                error = %err,
                "{MAX_AUTO_SLUG_LENGTH_ENV} is not a valid number; ignoring and falling through to the next auto-slug-length tier"
            );
            None
        }
    }
}

/// Precedence resolution given each tier's already-fetched value:
/// `override_len` beats `env_len` beats `config_len` beats
/// [`DEFAULT_MAX_AUTO_SLUG_LENGTH`]. A `Some(0)` at any tier counts as
/// absent (falls through), since a zero-length slug cap is never a
/// legitimate intent; whichever tier is the one actually rejected for
/// this reason is logged, naming that specific tier, before falling
/// through to the next one.
fn resolve_from_tiers(
    override_len: Option<usize>,
    env_len: Option<usize>,
    config_len: Option<usize>,
) -> usize {
    if let Some(n) = override_len {
        if n > 0 {
            return n;
        }
        tracing::warn!(
            "auto-slug-length override tier rejected: an explicit per-call max_auto_slug_length = 0 is not a legitimate cap; falling through to the next auto-slug-length tier"
        );
    }
    if let Some(n) = env_len {
        if n > 0 {
            return n;
        }
        tracing::warn!(
            "auto-slug-length env tier rejected: {MAX_AUTO_SLUG_LENGTH_ENV} = 0 is not a legitimate cap; falling through to the next auto-slug-length tier"
        );
    }
    if let Some(n) = config_len {
        if n > 0 {
            return n;
        }
        tracing::warn!(
            "auto-slug-length config tier rejected: [limits] max_auto_slug_length = 0 is not a legitimate cap; falling through to the compiled-in default"
        );
    }
    DEFAULT_MAX_AUTO_SLUG_LENGTH
}

/// Read `[limits] max_auto_slug_length` from the user-level `~/.mmcp/config.toml`, if present.
/// Mirrors the read-only, missing-file-or-section-means-`None` style already used by
/// `MmcpHome::resolve_author` for the same config file (see [`crate::home`]);
/// never errors, since a broken or absent user config must never fail slug generation.
/// A discovery or parse failure is logged before falling through, rather than discarded with no signal.
fn user_config_max_auto_slug_length() -> Option<usize> {
    let home = match crate::home::MmcpHome::discover() {
        Ok(home) => home,
        Err(err) => {
            tracing::warn!(
                error = %err,
                "failed to discover MmcpHome while resolving the auto-slug-length config tier; falling through to the next tier"
            );
            return None;
        }
    };
    let cfg = match home.load_user_config() {
        Ok(cfg) => cfg,
        Err(err) => {
            tracing::warn!(
                error = %err,
                "user config failed to parse while resolving the auto-slug-length config tier; falling through to the next tier"
            );
            return None;
        }
    };
    cfg.limits.and_then(|limits| limits.max_auto_slug_length)
}

/// Truncate `slug` to at most `max_len` bytes, landing on the last hyphen boundary at or before the cap
/// so the result never splits a word mid-character and never ends with a dangling hyphen.
/// Returns `slug` unchanged (cloned) when it is already within the cap.
///
/// `slug::slugify` output is pure ASCII (it transliterates Unicode before hyphenating),
/// so byte-slicing at `max_len` never lands mid-character.
/// When the first `max_len` bytes contain no hyphen at all (a single word longer than the cap),
/// this falls back to a hard cut at `max_len`,
/// the only case where the result can still end mid-word, since there is no boundary to land on.
fn truncate_slug_at_hyphen_boundary(slug: &str, max_len: usize) -> String {
    if slug.len() <= max_len {
        return slug.to_string();
    }
    let truncated = &slug[..max_len];
    match truncated.rfind('-') {
        Some(boundary) => truncated[..boundary].to_string(),
        None => truncated.to_string(),
    }
}

/// Return `filename` with its trailing `.md` / `.adoc` / `.asciidoc` extension stripped, if any.
/// Case-insensitive on the extension so `README.MD` and `Notes.ADOC` slim down the same as their lower-case siblings.
/// Returns the input unchanged when no known extension matches.
fn strip_known_import_extension(filename: &str) -> &str {
    let Some((stem, ext)) = filename.rsplit_once('.') else {
        return filename;
    };
    if stem.is_empty() || ext.is_empty() {
        return filename;
    }
    let ext_lower = ext.to_ascii_lowercase();
    // `.md` is the canonical on-disk extension; the adoc variants
    // come from the import-side bridge in `crate::import_adoc`.
    let md_ext = mmcp_core::conventions::MEMORY_EXTENSION
        .trim_start_matches('.')
        .to_ascii_lowercase();
    if ext_lower == md_ext {
        return stem;
    }
    if crate::import_adoc::ADOC_EXTENSIONS
        .iter()
        .any(|e| *e == ext_lower)
    {
        return stem;
    }
    filename
}

/// Parse a kind string into `MemoryKind` for SYNTAX only.
/// Delegates to the canonical [`MemoryKind::from_str`](std::str::FromStr) parser,
/// so every caller accepts exactly the same kind set (all eight, tracked kinds included)
/// as the archive filter and the GUI DTO converter.
/// Used directly by contexts that legitimately need every kind, such as archive re-import;
/// a plain memory CREATE / `edit --kind` entry point should call [`parse_creatable_kind`] instead,
/// which layers the create-time policy on top.
pub fn parse_kind(s: &str) -> Result<MemoryKind, ImportError> {
    Ok(s.parse::<MemoryKind>()?)
}

/// The five kinds a plain memory CREATE (or `edit --kind`) may target.
/// Tracked kinds are deliberately excluded: they are created through their own dedicated command
/// (`add_feature` / `add_issue` / `add_milestone`), which populates the structured metadata subtable
/// this path never does.
const CREATABLE_KINDS: &[MemoryKind] = &[
    MemoryKind::Rule,
    MemoryKind::Snapshot,
    MemoryKind::Log,
    MemoryKind::Reference,
    MemoryKind::Scratch,
];

/// Parse a kind string for `mmcp memory create` / `mmcp memory edit --kind`.
/// Delegates to [`parse_kind`] for syntax.
/// That gives a genuinely unknown kind the same error text as every other kind-parsing call site.
/// The result then goes through the create-time policy restriction against `CREATABLE_KINDS`.
/// A syntactically valid but tracked kind returns [`ImportError::NotACreatableKind`].
/// Rejecting a tracked kind here keeps `mmcp memory create` from writing a memory the tracked-kind tooling rejects.
pub fn parse_creatable_kind(s: &str) -> Result<MemoryKind, ImportError> {
    let kind = parse_kind(s)?;
    if CREATABLE_KINDS.contains(&kind) {
        Ok(kind)
    } else {
        Err(ImportError::NotACreatableKind {
            kind: kind.as_str().to_string(),
        })
    }
}

/// Resolve a group by UUID or slug.
pub async fn resolve_group(
    groups: &GroupIndex,
    identifier: &str,
) -> Result<GroupEntry, ImportError> {
    // Try UUID first.
    if let Ok(uuid) = Uuid::parse_str(identifier) {
        let id = GroupId::from_uuid(uuid);
        if let Some(entry) = groups.get(&id).await {
            return Ok(entry);
        }
    }
    // Fall back to slug lookup.
    let all = groups.list().await;
    for entry in all {
        if entry.manifest.slug == identifier {
            return Ok(entry);
        }
    }
    Err(ImportError::GroupNotFound(identifier.to_string()))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use mmcp_core::manifest::GroupManifest;
    use std::sync::Arc;
    use tempfile::TempDir;

    fn test_author() -> ResolvedAuthor {
        ResolvedAuthor {
            name: "test".to_string(),
            email: "test@test.invalid".to_string(),
        }
    }

    async fn test_backend() -> (Arc<NativeBackend>, RepoHandle, TempDir) {
        let tmp = TempDir::new().expect("tempdir");
        let backend = Arc::new(NativeBackend::new(tmp.path()).expect("backend"));
        let owner = mmcp_core::id::UserId::new();
        let group_id = GroupId::new();
        let manifest = GroupManifest::new_user_owned(group_id, "test", owner);
        let handle = backend.create_group_repo(&manifest).await.expect("create");
        (backend, handle, tmp)
    }

    #[test]
    fn validate_memory_slug_accepts_single_segment() {
        assert!(validate_memory_slug("hello").is_ok());
        assert!(validate_memory_slug("hello-world").is_ok());
        assert!(validate_memory_slug("a").is_ok());
        assert!(validate_memory_slug("foo-bar-baz-123").is_ok());
    }

    #[test]
    fn validate_memory_slug_rejects_invalid_single_segment() {
        assert!(validate_memory_slug("").is_err());
        assert!(validate_memory_slug("-leading").is_err());
        assert!(validate_memory_slug("trailing-").is_err());
        assert!(validate_memory_slug("UPPER").is_err());
        assert!(validate_memory_slug("has space").is_err());
        assert!(validate_memory_slug("double--hyphen").is_err());
        // Total-length cap: now MAX_SLUG_LENGTH (256). 257 chars
        // overflow even when each segment passes the per-segment
        // rules.
        assert!(validate_memory_slug(&"a".repeat(MAX_SLUG_LENGTH + 1)).is_err());
    }

    #[test]
    fn validate_memory_slug_accepts_paths() {
        // Multi-segment paths with `/` separators.
        assert!(validate_memory_slug("feedback/git/commit-phase").is_ok());
        assert!(validate_memory_slug("rules/testing").is_ok());
        assert!(validate_memory_slug("a/b/c/d/e/f/g/h").is_ok()); // 8 segments OK
    }

    #[test]
    fn validate_memory_slug_rejects_path_violations() {
        // Leading / trailing / empty / dotted segments.
        assert!(validate_memory_slug("/leading").is_err());
        assert!(validate_memory_slug("trailing/").is_err());
        assert!(validate_memory_slug("a//b").is_err());
        assert!(validate_memory_slug("a/../b").is_err());
        assert!(validate_memory_slug("a/./b").is_err());
        // Depth cap.
        assert!(validate_memory_slug("a/b/c/d/e/f/g/h/i").is_err());
        // Per-segment rules still apply.
        assert!(validate_memory_slug("ok/-bad").is_err());
        assert!(validate_memory_slug("ok/UPPER").is_err());
    }

    #[test]
    fn validate_slug_segment_rejects_slashes() {
        // Group / project slugs go through the segment validator;
        // a `/` in either is always invalid because they're not
        // path-bearing identifiers.
        assert!(validate_slug_segment("ok").is_ok());
        assert!(validate_slug_segment("with/slash").is_err());
    }

    #[test]
    fn slugify_filename_works() {
        assert_eq!(slugify_filename("Hello World.md"), "hello-world");
        assert_eq!(
            slugify_filename("global_coding_rules.md"),
            "global-coding-rules"
        );
        assert_eq!(slugify_filename("already-good"), "already-good");
        assert_eq!(slugify_filename("  spaces  .md"), "spaces");
    }

    #[test]
    fn slugify_filename_strips_adoc_and_asciidoc_extensions() {
        // The import bridge converts adoc sources to markdown before storage,
        // but the slug is still derived from the original file name.
        // Without this strip, an operator importing `coding-rules.adoc` would end up
        // with the `coding-rules-adoc` slug, which carries the source format
        // into a field that should only reflect the memory's identity.
        assert_eq!(slugify_filename("coding-rules.adoc"), "coding-rules");
        assert_eq!(slugify_filename("CODING-RULES.ADOC"), "coding-rules");
        assert_eq!(slugify_filename("team/guide.asciidoc"), "team-guide");
        assert_eq!(slugify_filename("Team Guide.AsciiDoc"), "team-guide");
    }

    #[test]
    fn slugify_filename_caps_absurdly_long_titles_at_a_hyphen_boundary() {
        // Real auto-derived issue titles, both filed with zero length cap
        // and both far past any sane directory-name length.
        // Pinned via an explicit override (rather than the ambient `slugify_filename` default)
        // so the assertion is deterministic regardless of this machine's `MMCP_MAX_AUTO_SLUG_LENGTH`
        // env var or `~/.mmcp/config.toml` `[limits]` override.
        let cargo_build_title = "cargo-build-s-windows-exe-stash-step-fails-across-drives-cargo-target-dir-on-a-different-drive-than-the-project.md";
        let list_memories_title = "list-memories-corrupts-feature-kind-records-into-kind-rule-null-null-instead-of-failing-loudly-root-cause-status-requested-rejected-by-the-current-featurestatus-enum.md";
        let cap = DEFAULT_MAX_AUTO_SLUG_LENGTH;

        for title in [cargo_build_title, list_memories_title] {
            let result = slugify_filename_with_cap(title, Some(cap));
            assert!(
                result.len() <= cap,
                "slug '{result}' ({} chars) exceeds the {cap}-char cap",
                result.len()
            );
            assert!(
                !result.ends_with('-'),
                "slug '{result}' ends with a dangling hyphen"
            );
            assert!(!result.is_empty(), "slug must not be empty");
        }
    }

    #[test]
    fn resolve_from_tiers_prefers_explicit_override() {
        assert_eq!(resolve_from_tiers(Some(10), Some(20), Some(30)), 10);
    }

    #[test]
    fn resolve_from_tiers_falls_back_to_env_then_config_then_default() {
        assert_eq!(resolve_from_tiers(None, Some(20), Some(30)), 20);
        assert_eq!(resolve_from_tiers(None, None, Some(30)), 30);
        assert_eq!(
            resolve_from_tiers(None, None, None),
            DEFAULT_MAX_AUTO_SLUG_LENGTH
        );
    }

    #[test]
    fn resolve_from_tiers_treats_zero_as_absent_at_every_tier() {
        // A zero-length cap is never a legitimate intent, so it falls
        // through to the next tier exactly like `None` would.
        assert_eq!(resolve_from_tiers(Some(0), Some(20), Some(30)), 20);
        assert_eq!(resolve_from_tiers(Some(0), Some(0), Some(30)), 30);
        assert_eq!(
            resolve_from_tiers(Some(0), Some(0), Some(0)),
            DEFAULT_MAX_AUTO_SLUG_LENGTH
        );
    }

    /// Minimal `tracing::Subscriber` counting `WARN`-level events, so
    /// a rejected auto-slug-length tier can be asserted to actually
    /// log instead of silently discarding the bad value.
    struct WarnCounter(Arc<std::sync::atomic::AtomicUsize>);

    impl tracing::Subscriber for WarnCounter {
        fn enabled(&self, metadata: &tracing::Metadata<'_>) -> bool {
            *metadata.level() == tracing::Level::WARN
        }
        fn new_span(&self, _span: &tracing::span::Attributes<'_>) -> tracing::span::Id {
            tracing::span::Id::from_u64(1)
        }
        fn record(&self, _span: &tracing::span::Id, _values: &tracing::span::Record<'_>) {}
        fn record_follows_from(&self, _span: &tracing::span::Id, _follows: &tracing::span::Id) {}
        fn event(&self, event: &tracing::Event<'_>) {
            if *event.metadata().level() == tracing::Level::WARN {
                self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }
        }
        fn enter(&self, _span: &tracing::span::Id) {}
        fn exit(&self, _span: &tracing::span::Id) {}
    }

    #[test]
    fn resolve_from_tiers_warns_for_the_config_tier_only_when_actually_reached() {
        let count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let subscriber = WarnCounter(count.clone());

        // The config tier is Some(0) in both calls below, but the
        // first call never reaches it (override wins), so it must
        // never warn; the second call falls through to it, so it
        // must warn exactly once.
        tracing::subscriber::with_default(subscriber, || {
            assert_eq!(resolve_from_tiers(Some(10), None, Some(0)), 10);
        });
        assert_eq!(
            count.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "the zero config tier was never consulted, so it must not warn"
        );

        let count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let subscriber = WarnCounter(count.clone());
        tracing::subscriber::with_default(subscriber, || {
            assert_eq!(
                resolve_from_tiers(None, None, Some(0)),
                DEFAULT_MAX_AUTO_SLUG_LENGTH
            );
        });
        assert_eq!(
            count.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "falling through the zero config tier must log exactly one warning"
        );
    }

    #[test]
    fn resolve_from_tiers_warns_when_the_override_tier_is_zero() {
        let count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let subscriber = WarnCounter(count.clone());

        tracing::subscriber::with_default(subscriber, || {
            assert_eq!(
                resolve_from_tiers(Some(0), Some(20), Some(30)),
                20,
                "a rejected Some(0) override must fall through to the env tier"
            );
        });
        assert_eq!(
            count.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "a rejected Some(0) override tier must log exactly one warning"
        );
    }

    #[test]
    fn resolve_from_tiers_warns_when_the_env_tier_is_zero() {
        let count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let subscriber = WarnCounter(count.clone());

        tracing::subscriber::with_default(subscriber, || {
            assert_eq!(
                resolve_from_tiers(None, Some(0), Some(30)),
                30,
                "a rejected Some(0) env tier must fall through to the config tier"
            );
        });
        assert_eq!(
            count.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "a rejected Some(0) env tier must log exactly one warning"
        );
    }

    #[test]
    fn parse_env_auto_slug_length_warns_on_malformed_value() {
        let count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let subscriber = WarnCounter(count.clone());

        let result = tracing::subscriber::with_default(subscriber, || {
            parse_env_auto_slug_length(Some("not-a-number"))
        });

        assert_eq!(
            result, None,
            "a malformed env var must be treated as absent, not fabricated into a number"
        );
        assert_eq!(
            count.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "a malformed env var must log exactly one warning instead of being silently discarded"
        );
    }

    #[test]
    fn parse_env_auto_slug_length_accepts_a_valid_value_without_warning() {
        let count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let subscriber = WarnCounter(count.clone());

        let result = tracing::subscriber::with_default(subscriber, || {
            parse_env_auto_slug_length(Some("42"))
        });

        assert_eq!(result, Some(42));
        assert_eq!(
            count.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "a valid env var must never warn"
        );
    }

    #[test]
    fn parse_kind_round_trips() {
        assert_eq!(parse_kind("rule").unwrap(), MemoryKind::Rule);
        assert_eq!(parse_kind("reference").unwrap(), MemoryKind::Reference);
        // Delegating to the canonical MemoryKind::from_str widens this syntax-only parser
        // to accept every kind, matching the archive filter and GUI decoders.
        // A memory CREATE / `edit --kind` entry point must go through `parse_creatable_kind` instead,
        // which re-applies the narrower policy below.
        assert_eq!(parse_kind("feature").unwrap(), MemoryKind::Feature);
        assert_eq!(parse_kind("issue").unwrap(), MemoryKind::Issue);
        assert!(parse_kind("bogus").is_err());
    }

    #[test]
    fn parse_creatable_kind_accepts_the_five_non_tracked_kinds() {
        assert_eq!(parse_creatable_kind("rule").unwrap(), MemoryKind::Rule);
        assert_eq!(
            parse_creatable_kind("snapshot").unwrap(),
            MemoryKind::Snapshot
        );
        assert_eq!(parse_creatable_kind("log").unwrap(), MemoryKind::Log);
        assert_eq!(
            parse_creatable_kind("reference").unwrap(),
            MemoryKind::Reference
        );
        assert_eq!(
            parse_creatable_kind("scratch").unwrap(),
            MemoryKind::Scratch
        );
    }

    #[test]
    fn parse_creatable_kind_rejects_every_tracked_kind() {
        for tracked in ["feature", "issue", "milestone"] {
            let err = parse_creatable_kind(tracked)
                .expect_err("a tracked kind must not be creatable via memory create/edit");
            match &err {
                ImportError::NotACreatableKind { kind } => assert_eq!(kind, tracked),
                other => panic!("unexpected error for {tracked}: {other:?}"),
            }
        }
    }

    #[test]
    fn parse_creatable_kind_still_rejects_a_syntactically_unknown_kind() {
        assert!(parse_creatable_kind("bogus").is_err());
    }

    #[tokio::test]
    async fn import_with_frontmatter_succeeds() {
        let (backend, handle, _tmp) = test_backend().await;
        let content =
            "+++\nname = \"test\"\ndescription = \"a test\"\nkind = \"rule\"\n+++\n\nBody here.\n";
        let author = test_author();
        let result = import_memory(&backend, &handle, "test-mem", content, None, &author, false)
            .await
            .expect("import");
        assert_eq!(result.slug, "test-mem");
        assert!(!result.commit_id.is_empty());
    }

    #[tokio::test]
    async fn import_with_synth_frontmatter_succeeds() {
        let (backend, handle, _tmp) = test_backend().await;
        let author = test_author();
        let content = "Just plain markdown body.\n";
        let synth = Some(SynthFrontmatter {
            name: "plain".to_string(),
            description: "imported plain".to_string(),
            kind: MemoryKind::Reference,
        });
        let result = import_memory(
            &backend,
            &handle,
            "plain-mem",
            content,
            synth,
            &author,
            false,
        )
        .await
        .expect("import");
        assert_eq!(result.slug, "plain-mem");
    }

    #[tokio::test]
    async fn import_without_frontmatter_or_synth_fails() {
        let (backend, handle, _tmp) = test_backend().await;
        let author = test_author();
        let content = "No frontmatter here.\n";
        let err = import_memory(&backend, &handle, "bad", content, None, &author, false)
            .await
            .unwrap_err();
        assert!(matches!(err, ImportError::MissingFrontmatter));
    }

    const SAMPLE_RENDERED: &str = "+++\nname = \"sample\"\ndescription = \"s\"\nkind = \"rule\"\nmandatory = false\ntags = []\n+++\nBody text.\n";

    #[tokio::test]
    async fn import_memory_mints_uuid_and_lands_under_slug() {
        let (backend, handle, _tmp) = test_backend().await;
        let author = test_author();
        let result = import_memory(
            &backend,
            &handle,
            "minted",
            SAMPLE_RENDERED,
            None,
            &author,
            false,
        )
        .await
        .expect("import");
        assert_eq!(result.slug, "minted");
        // Resolving the slug finds exactly one entry, the one just minted,
        // and its path sits under the slug dir.
        let resolved = resolve_memory(&backend, &handle, Some("minted"), None)
            .await
            .expect("resolve");
        assert_eq!(resolved.slug, "minted");
        assert!(
            resolved.path.starts_with("memories/minted/"),
            "expected two-level path, got {}",
            resolved.path
        );
    }

    #[tokio::test]
    async fn import_memory_repeated_produces_sibling_uuids() {
        // Duplicate slugs are legal: each import mints a fresh UUIDv7,
        // and lands as a sibling of the prior file under the same slug directory.
        let (backend, handle, _tmp) = test_backend().await;
        let author = test_author();
        let first = import_memory(
            &backend,
            &handle,
            "twin",
            SAMPLE_RENDERED,
            None,
            &author,
            false,
        )
        .await
        .expect("first");
        let second = import_memory(
            &backend,
            &handle,
            "twin",
            SAMPLE_RENDERED,
            None,
            &author,
            false,
        )
        .await
        .expect("second twin");
        assert_eq!(first.slug, second.slug);
        assert_ne!(first.commit_id, second.commit_id);
        let err = resolve_memory(&backend, &handle, Some("twin"), None)
            .await
            .expect_err("ambiguous");
        assert!(matches!(
            err,
            ImportError::MemoryAmbiguous { slug, candidates } if slug == "twin" && candidates.len() == 2
        ));
    }

    #[tokio::test]
    async fn import_memory_rejects_pinned_id_collision_without_override() {
        // When the frontmatter carries an explicit `id`, a second
        // import with the same slug + id must refuse unless the
        // caller opts into replace via `override_existing`.
        let (backend, handle, _tmp) = test_backend().await;
        let author = test_author();
        let id = Uuid::now_v7();
        let pinned = format!(
            "+++\nid = \"{id}\"\nname = \"pinned\"\ndescription = \"s\"\nkind = \"rule\"\n+++\nbody\n"
        );
        import_memory(&backend, &handle, "taken", &pinned, None, &author, false)
            .await
            .expect("seed");
        let err = import_memory(&backend, &handle, "taken", &pinned, None, &author, false)
            .await
            .expect_err("second create must refuse");
        assert!(matches!(err, ImportError::MemoryAlreadyExists { slug } if slug == "taken"));
    }

    async fn seed_two_level_memory(
        backend: &NativeBackend,
        handle: &RepoHandle,
        slug: &str,
        id: Uuid,
        author: &ResolvedAuthor,
    ) {
        let body = format!(
            "+++\nid = \"{id}\"\nname = \"m\"\ndescription = \"m\"\nkind = \"rule\"\n+++\nbody\n"
        );
        backend
            .write_commit(
                handle,
                CommitSpec::mmcp_commit(
                    format!("seed {slug}/{id}"),
                    vec![(
                        mmcp_core::conventions::memory_path(slug, MemoryId::from_uuid(id)),
                        Some(body.into_bytes()),
                    )],
                    &author.name,
                    &author.email,
                ),
            )
            .await
            .expect("seed commit");
    }

    #[tokio::test]
    async fn resolve_memory_requires_slug_or_id() {
        let (backend, handle, _tmp) = test_backend().await;
        let err = resolve_memory(&backend, &handle, None, None)
            .await
            .expect_err("neither arg");
        assert!(matches!(err, ImportError::ResolveArgsMissing));
    }

    #[tokio::test]
    async fn resolve_memory_by_slug_hits_two_level_layout() {
        let (backend, handle, _tmp) = test_backend().await;
        let author = test_author();
        let id = Uuid::now_v7();
        seed_two_level_memory(&backend, &handle, "rt", id, &author).await;

        let resolved = resolve_memory(&backend, &handle, Some("rt"), None)
            .await
            .expect("resolve");
        assert_eq!(resolved.slug, "rt");
        assert_eq!(resolved.id, id);
        assert_eq!(resolved.path, format!("memories/rt/{id}.md"));
    }

    #[tokio::test]
    async fn resolve_memory_by_slug_is_ambiguous_when_multiple_ids_exist() {
        let (backend, handle, _tmp) = test_backend().await;
        let author = test_author();
        let id1 = Uuid::now_v7();
        let id2 = Uuid::now_v7();
        seed_two_level_memory(&backend, &handle, "dup", id1, &author).await;
        seed_two_level_memory(&backend, &handle, "dup", id2, &author).await;

        let err = resolve_memory(&backend, &handle, Some("dup"), None)
            .await
            .expect_err("ambiguous");
        let ImportError::MemoryAmbiguous { slug, candidates } = err else {
            panic!("expected MemoryAmbiguous, got different variant");
        };
        assert_eq!(slug, "dup");
        assert_eq!(candidates.len(), 2);
        assert!(candidates.contains(&id1));
        assert!(candidates.contains(&id2));
    }

    #[tokio::test]
    async fn resolve_memory_by_id_finds_slug_directory() {
        let (backend, handle, _tmp) = test_backend().await;
        let author = test_author();
        let id = Uuid::now_v7();
        seed_two_level_memory(&backend, &handle, "byid", id, &author).await;

        let resolved = resolve_memory(&backend, &handle, None, Some(id))
            .await
            .expect("resolve by id");
        assert_eq!(resolved.slug, "byid");
        assert_eq!(resolved.id, id);
    }

    #[tokio::test]
    async fn resolve_memory_slug_and_id_detects_mismatch() {
        let (backend, handle, _tmp) = test_backend().await;
        let author = test_author();
        let real = Uuid::now_v7();
        let other = Uuid::now_v7();
        seed_two_level_memory(&backend, &handle, "mm", real, &author).await;

        // Caller supplies `other`; the two-level path `memories/mm/<other>.md` doesn't exist,
        // so the resolver falls back to the legacy flat path `memories/mm.md`.
        // That also doesn't exist here, so the error should be MemoryNotFound with both keys populated.
        let err = resolve_memory(&backend, &handle, Some("mm"), Some(other))
            .await
            .expect_err("not found");
        assert!(matches!(
            err,
            ImportError::MemoryNotFound { slug: Some(s), id: Some(i) } if s == "mm" && i == other
        ));
    }

    #[tokio::test]
    async fn resolve_memory_not_found_returns_typed_error() {
        let (backend, handle, _tmp) = test_backend().await;
        let err = resolve_memory(&backend, &handle, Some("missing"), None)
            .await
            .expect_err("missing");
        assert!(matches!(
            err,
            ImportError::MemoryNotFound { slug: Some(s), id: None } if s == "missing"
        ));
    }

    #[tokio::test]
    async fn read_frontmatter_returns_metadata_for_existing_memory() {
        let (backend, handle, _tmp) = test_backend().await;
        let author = test_author();
        let content = "+++\nname = \"fm\"\ndescription = \"frontmatter only\"\nkind = \"rule\"\nmandatory = true\ntags = [\"test\", \"fm\"]\n+++\n\nA much longer body block that should not influence the frontmatter\nread path. Allocating this body is the cost we want to skip in the\nfollow-up fence-slice optimisation.\n";
        let result = import_memory(&backend, &handle, "fm-test", content, None, &author, false)
            .await
            .expect("import");
        let id = result.id;

        let (resolved, fm) = read_frontmatter(&backend, &handle, Some("fm-test"), None)
            .await
            .expect("read frontmatter");
        assert_eq!(resolved.slug, "fm-test");
        assert_eq!(resolved.id, id);
        assert_eq!(fm.id, Some(id));
        assert_eq!(fm.name, "fm");
        assert_eq!(fm.description, "frontmatter only");
        assert_eq!(fm.kind, MemoryKind::Rule);
        assert!(fm.mandatory);
        assert_eq!(fm.tags, vec!["test", "fm"]);
    }

    #[tokio::test]
    async fn read_frontmatter_propagates_resolve_errors() {
        let (backend, handle, _tmp) = test_backend().await;
        let err = read_frontmatter(&backend, &handle, Some("missing"), None)
            .await
            .expect_err("missing");
        assert!(matches!(
            err,
            ImportError::MemoryNotFound { slug: Some(s), id: None } if s == "missing"
        ));
    }

    #[tokio::test]
    async fn read_frontmatter_matches_full_read_for_yaml_fenced_files() {
        let (backend, handle, _tmp) = test_backend().await;
        let author = test_author();
        let id = Uuid::now_v7();
        // Seed a YAML-fenced memory directly so the parser's `---` branch is exercised end-to-end.
        // Universal frontmatter support means YAML must round-trip through the same primitive.
        let yaml_body = format!(
            "---\nid: \"{id}\"\nname: yam\ndescription: yaml fenced\nkind: rule\n---\nBody after yaml fence.\n"
        );
        backend
            .write_commit(
                &handle,
                CommitSpec::mmcp_commit(
                    format!("seed yam/{id}"),
                    vec![(
                        mmcp_core::conventions::memory_path("yam", MemoryId::from_uuid(id)),
                        Some(yaml_body.into_bytes()),
                    )],
                    &author.name,
                    &author.email,
                ),
            )
            .await
            .expect("seed");

        let (_, fm) = read_frontmatter(&backend, &handle, Some("yam"), Some(id))
            .await
            .expect("read");
        assert_eq!(fm.id, Some(id));
        assert_eq!(fm.name, "yam");
        assert_eq!(fm.description, "yaml fenced");
        assert_eq!(fm.kind, MemoryKind::Rule);
    }

    #[tokio::test]
    async fn read_frontmatters_in_group_returns_all_files() {
        let (backend, handle, _tmp) = test_backend().await;
        let author = test_author();
        let id_a = Uuid::now_v7();
        let id_b = Uuid::now_v7();
        seed_two_level_memory(&backend, &handle, "alpha", id_a, &author).await;
        seed_two_level_memory(&backend, &handle, "beta", id_b, &author).await;

        let entries = read_frontmatters_in_group(&backend, &handle, &Rev::head())
            .await
            .expect("batch read");
        assert_eq!(entries.len(), 2);
        let mut by_slug: std::collections::HashMap<String, &MemoryFrontmatterEntry> =
            std::collections::HashMap::new();
        for entry in &entries {
            by_slug.insert(entry.file.slug.clone(), entry);
        }
        let alpha = by_slug.get("alpha").expect("alpha entry");
        let beta = by_slug.get("beta").expect("beta entry");
        assert_eq!(alpha.file.id, id_a);
        assert_eq!(beta.file.id, id_b);
        let alpha_fm = alpha.frontmatter.as_ref().expect("alpha frontmatter");
        let beta_fm = beta.frontmatter.as_ref().expect("beta frontmatter");
        assert_eq!(alpha_fm.id, Some(id_a));
        assert_eq!(beta_fm.id, Some(id_b));
    }

    #[tokio::test]
    async fn read_frontmatters_in_group_isolates_per_file_parse_errors() {
        let (backend, handle, _tmp) = test_backend().await;
        let author = test_author();
        let good_id = Uuid::now_v7();
        let bad_id = Uuid::now_v7();
        seed_two_level_memory(&backend, &handle, "good", good_id, &author).await;
        // Seed a malformed file: no frontmatter fences, just plain text.
        // The fan-out helper must surface the parse error per-entry
        // rather than aborting the whole listing.
        backend
            .write_commit(
                &handle,
                CommitSpec::mmcp_commit(
                    format!("seed bad/{bad_id}"),
                    vec![(
                        mmcp_core::conventions::memory_path("bad", MemoryId::from_uuid(bad_id)),
                        Some(b"plain text without frontmatter\n".to_vec()),
                    )],
                    &author.name,
                    &author.email,
                ),
            )
            .await
            .expect("seed bad");

        let entries = read_frontmatters_in_group(&backend, &handle, &Rev::head())
            .await
            .expect("batch read");
        assert_eq!(entries.len(), 2);
        for entry in &entries {
            match entry.file.slug.as_str() {
                "good" => {
                    let fm = entry.frontmatter.as_ref().expect("good parses");
                    assert_eq!(fm.id, Some(good_id));
                }
                "bad" => {
                    let err = entry
                        .frontmatter
                        .as_ref()
                        .expect_err("bad surfaces parse error");
                    assert!(matches!(err, ImportError::Parse(_)));
                }
                other => panic!("unexpected slug {other}"),
            }
        }
    }

    /// Regression guard for the zero-copy UTF-8 fix at `read_frontmatter_at`: a blob that is not
    /// valid UTF-8 must surface a typed [`ImportError::NotUtf8`] per-entry, never silently
    /// substitute the replacement character (the previous `String::from_utf8_lossy` behavior),
    /// which would corrupt frontmatter/body content instead of reporting the truncation.
    #[tokio::test]
    async fn read_frontmatters_in_group_reports_invalid_utf8() {
        let (backend, handle, _tmp) = test_backend().await;
        let author = test_author();
        let bad_id = Uuid::now_v7();
        // 0x80 alone is a lone UTF-8 continuation byte: never valid at any position.
        let invalid_bytes: Vec<u8> = vec![0x2b, 0x2b, 0x2b, 0x0a, 0x80, 0x0a];
        backend
            .write_commit(
                &handle,
                CommitSpec::mmcp_commit(
                    format!("seed not-utf8/{bad_id}"),
                    vec![(
                        mmcp_core::conventions::memory_path(
                            "not-utf8",
                            MemoryId::from_uuid(bad_id),
                        ),
                        Some(invalid_bytes),
                    )],
                    &author.name,
                    &author.email,
                ),
            )
            .await
            .expect("seed invalid-utf8 memory");

        let entries = read_frontmatters_in_group(&backend, &handle, &Rev::head())
            .await
            .expect("batch read");
        let entry = entries
            .iter()
            .find(|e| e.file.slug == "not-utf8")
            .expect("not-utf8 entry present");
        let err = entry
            .frontmatter
            .as_ref()
            .expect_err("invalid UTF-8 surfaces as a typed error");
        assert!(matches!(err, ImportError::NotUtf8 { .. }));
    }

    #[tokio::test]
    async fn import_then_read_round_trips() {
        let (backend, handle, _tmp) = test_backend().await;
        let author = test_author();
        let content = "+++\nname = \"rt\"\ndescription = \"round trip\"\nkind = \"rule\"\nmandatory = true\ntags = [\"test\"]\n+++\n\nRound trip body.\n";
        import_memory(&backend, &handle, "rt-test", content, None, &author, false)
            .await
            .expect("import");

        let resolved = resolve_memory(&backend, &handle, Some("rt-test"), None)
            .await
            .expect("resolve imported slug");
        let bytes = backend
            .read_file(&handle, &resolved.path, &Rev::Branch("main".to_string()))
            .await
            .expect("read back");
        let text = std::str::from_utf8(&bytes).expect("utf8");
        let parsed = MemoryFile::parse(text).expect("parse back");
        assert_eq!(parsed.frontmatter.name, "rt");
        assert_eq!(parsed.frontmatter.description, "round trip");
        assert_eq!(parsed.frontmatter.kind, MemoryKind::Rule);
        assert!(parsed.frontmatter.mandatory);
        assert!(parsed.body.contains("Round trip body"));
        assert_eq!(parsed.frontmatter.id, Some(resolved.id));
    }

    #[tokio::test]
    async fn list_memory_slug_dirs_walks_nested_paths() {
        // A memory at `feedback/git/scope/<uuid>.md` and one at the flat `legacy/<uuid>.md`,
        // should both surface as separate leaf slug directories.
        let (backend, handle, _tmp) = test_backend().await;
        let author = test_author();
        let nested_id = Uuid::now_v7();
        let flat_id = Uuid::now_v7();
        seed_raw(
            &backend,
            &handle,
            &format!("memories/feedback/git/scope/{nested_id}.md"),
            "+++\nname = \"x\"\ndescription = \"x\"\nkind = \"rule\"\n+++\n\n",
            &author,
        )
        .await;
        seed_raw(
            &backend,
            &handle,
            &format!("memories/legacy/{flat_id}.md"),
            "+++\nname = \"y\"\ndescription = \"y\"\nkind = \"rule\"\n+++\n\n",
            &author,
        )
        .await;

        let dirs = list_memory_slug_dirs(&backend, &handle, &Rev::head())
            .await
            .expect("list");
        let slugs: Vec<_> = dirs.iter().map(|d| d.slug.as_str()).collect();
        assert!(slugs.contains(&"feedback/git/scope"), "got: {slugs:?}");
        assert!(slugs.contains(&"legacy"), "got: {slugs:?}");
    }

    #[tokio::test]
    async fn list_all_memory_files_recurses_into_nested_paths() {
        let (backend, handle, _tmp) = test_backend().await;
        let author = test_author();
        let id = Uuid::now_v7();
        let path = format!("memories/feedback/git/{id}.md");
        seed_raw(
            &backend,
            &handle,
            &path,
            &format!(
                "+++\nname = \"n\"\ndescription = \"d\"\nkind = \"rule\"\nid = \"{id}\"\n+++\n\nbody"
            ),
            &author,
        )
        .await;
        let files = list_all_memory_files(&backend, &handle, &Rev::head())
            .await
            .expect("list");
        let hit = files
            .iter()
            .find(|f| f.id == id)
            .expect("nested memory surfaces in flat enumeration");
        assert_eq!(hit.slug, "feedback/git");
        assert_eq!(hit.path, path);
    }

    #[tokio::test]
    async fn count_all_memory_files_matches_list_all_memory_files_len() {
        let (backend, handle, _tmp) = test_backend().await;
        let author = test_author();
        // Nested slug.
        let nested_id = Uuid::now_v7();
        seed_raw(
            &backend,
            &handle,
            &format!("memories/feedback/git/{nested_id}.md"),
            "+++\nname = \"n\"\ndescription = \"d\"\nkind = \"rule\"\n+++\n\n",
            &author,
        )
        .await;
        // Flat slug with a stray non-UUID sibling that must not count.
        let flat_id = Uuid::now_v7();
        seed_raw(
            &backend,
            &handle,
            &format!("memories/legacy/{flat_id}.md"),
            "+++\nname = \"f\"\ndescription = \"f\"\nkind = \"rule\"\n+++\n\n",
            &author,
        )
        .await;
        seed_raw(
            &backend,
            &handle,
            "memories/legacy/not-a-uuid.md",
            "+++\nname = \"stray\"\ndescription = \"stray\"\nkind = \"rule\"\n+++\n\n",
            &author,
        )
        .await;
        // Hand-crafted root-level file: `list_all_memory_files` never
        // surfaces it (`list_memory_slug_dirs` filters `!slug.is_empty()`),
        // so the count must agree by construction, not by a parallel
        // filter that could silently diverge from it.
        let root_id = Uuid::now_v7();
        seed_raw(
            &backend,
            &handle,
            &format!("memories/{root_id}.md"),
            "+++\nname = \"r\"\ndescription = \"r\"\nkind = \"rule\"\n+++\n\n",
            &author,
        )
        .await;

        let files = list_all_memory_files(&backend, &handle, &Rev::head())
            .await
            .expect("list");
        let count = count_all_memory_files(&backend, &handle, &Rev::head())
            .await
            .expect("count");

        assert_eq!(
            count,
            files.len(),
            "count must match the full listing's length exactly"
        );
        assert_eq!(count, 2, "only the two UUID-named leaf files count");
    }

    /// Regression guard for [`count_all_memory_files_over`]: it walks the
    /// group's memory tree exactly once, regardless of how many slug
    /// directories exist, instead of looping a per-slug `list_tree` call
    /// (`count_slug_entries`'s shape) once per slug.
    #[tokio::test]
    async fn count_all_memory_files_over_walks_the_tree_exactly_once() {
        let call_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counted = Arc::clone(&call_count);
        let synthetic_dirs = (0..25)
            .map(|i| MemorySlugDir {
                slug: format!("bulk-{i}"),
                dir: format!("memories/bulk-{i}"),
                filenames: vec![format!("{}.md", Uuid::now_v7())],
            })
            .collect::<Vec<_>>();
        let expected_len = synthetic_dirs.len();

        let count = count_all_memory_files_over(move || {
            counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            async move { Ok(synthetic_dirs) }
        })
        .await
        .expect("count");

        assert_eq!(
            call_count.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "the walk seam must be called exactly once regardless of slug directory count"
        );
        assert_eq!(count, expected_len);
    }

    #[tokio::test]
    async fn resolve_memory_finds_nested_paths_by_id() {
        let (backend, handle, _tmp) = test_backend().await;
        let author = test_author();
        let id = Uuid::now_v7();
        seed_raw(
            &backend,
            &handle,
            &format!("memories/rules/testing/{id}.md"),
            &format!(
                "+++\nname = \"t\"\ndescription = \"t\"\nkind = \"rule\"\nid = \"{id}\"\n+++\n\nbody"
            ),
            &author,
        )
        .await;
        let resolved = resolve_memory(&backend, &handle, None, Some(id))
            .await
            .expect("resolve by id");
        assert_eq!(resolved.slug, "rules/testing");
        assert_eq!(resolved.id, id);
    }

    #[tokio::test]
    async fn move_memory_path_relocates_slug() {
        let (backend, handle, _tmp) = test_backend().await;
        let author = test_author();
        let content = "+++\nname = \"m\"\ndescription = \"m\"\nkind = \"rule\"\n+++\n\nMove me.\n";
        let imported = import_memory(&backend, &handle, "old-slug", content, None, &author, false)
            .await
            .expect("import");

        let outcome = move_memory_path(
            &backend,
            &handle,
            Some("old-slug"),
            Some(imported.id),
            "new/path/leaf",
            &author,
            None,
        )
        .await
        .expect("move");
        assert_eq!(outcome.id, imported.id);
        assert_eq!(outcome.new_slug, "new/path/leaf");
        assert!(!outcome.commit_id.is_empty());

        // Old path is gone; new path resolves and the body was
        // preserved verbatim.
        assert!(matches!(
            backend
                .read_file(&handle, &outcome.old_path, &Rev::head())
                .await,
            Err(GitError::PathNotFound(_))
        ));
        let resolved = resolve_memory(&backend, &handle, None, Some(imported.id))
            .await
            .expect("resolve new");
        assert_eq!(resolved.slug, "new/path/leaf");
        let bytes = backend
            .read_file(&handle, &resolved.path, &Rev::head())
            .await
            .expect("read");
        let text = std::str::from_utf8(&bytes).expect("utf8");
        assert!(text.contains("Move me."));
    }

    #[tokio::test]
    async fn move_memory_path_same_slug_is_noop() {
        let (backend, handle, _tmp) = test_backend().await;
        let author = test_author();
        let content = "+++\nname = \"s\"\ndescription = \"s\"\nkind = \"rule\"\n+++\n\n";
        let imported = import_memory(&backend, &handle, "stay", content, None, &author, false)
            .await
            .expect("import");
        let outcome = move_memory_path(
            &backend,
            &handle,
            Some("stay"),
            Some(imported.id),
            "stay",
            &author,
            None,
        )
        .await
        .expect("noop move");
        assert_eq!(outcome.commit_id, "");
        assert_eq!(outcome.old_path, outcome.new_path);
    }

    #[tokio::test]
    async fn move_memory_path_validates_target() {
        let (backend, handle, _tmp) = test_backend().await;
        let author = test_author();
        let content = "+++\nname = \"v\"\ndescription = \"v\"\nkind = \"rule\"\n+++\n\n";
        import_memory(&backend, &handle, "src", content, None, &author, false)
            .await
            .expect("import");
        let err = move_memory_path(
            &backend,
            &handle,
            Some("src"),
            None,
            "bad//path",
            &author,
            None,
        )
        .await
        .expect_err("invalid target slug");
        assert!(matches!(err, ImportError::InvalidSlug(_)));
    }

    #[tokio::test]
    async fn move_memory_path_rejects_oversized_message() {
        let (backend, handle, _tmp) = test_backend().await;
        let author = test_author();
        let content = "+++\nname = \"m\"\ndescription = \"m\"\nkind = \"rule\"\n+++\n\n";
        let imported = import_memory(&backend, &handle, "msg-src", content, None, &author, false)
            .await
            .expect("import");
        let oversized = "a".repeat(mmcp_core::memory::MAX_MESSAGE_LENGTH + 1);
        let err = move_memory_path(
            &backend,
            &handle,
            Some("msg-src"),
            Some(imported.id),
            "msg-dst",
            &author,
            Some(&oversized),
        )
        .await
        .expect_err("oversized message rejected");
        assert!(matches!(err, ImportError::FieldTooLong(_)));
        // Rejected before any commit: the memory is still at its
        // original slug.
        let resolved = resolve_memory(&backend, &handle, None, Some(imported.id))
            .await
            .expect("still resolvable");
        assert_eq!(resolved.slug, "msg-src");
    }

    #[tokio::test]
    async fn move_memory_path_accepts_message_within_bound() {
        let (backend, handle, _tmp) = test_backend().await;
        let author = test_author();
        let content = "+++\nname = \"m\"\ndescription = \"m\"\nkind = \"rule\"\n+++\n\n";
        let imported = import_memory(
            &backend,
            &handle,
            "msg-src-ok",
            content,
            None,
            &author,
            false,
        )
        .await
        .expect("import");
        let bounded = "a".repeat(mmcp_core::memory::MAX_MESSAGE_LENGTH);
        let outcome = move_memory_path(
            &backend,
            &handle,
            Some("msg-src-ok"),
            Some(imported.id),
            "msg-dst-ok",
            &author,
            Some(&bounded),
        )
        .await
        .expect("bounded message accepted");
        assert_eq!(outcome.new_slug, "msg-dst-ok");
        assert!(!outcome.commit_id.is_empty());
    }

    /// Seed an arbitrary file at an arbitrary path.
    /// Used by the addressing-mode tests to construct hand-crafted layouts
    /// the regular `import_memory` path won't produce on its own.
    async fn seed_raw(
        backend: &NativeBackend,
        handle: &RepoHandle,
        path: &str,
        body: &str,
        author: &ResolvedAuthor,
    ) {
        backend
            .write_commit(
                handle,
                CommitSpec::mmcp_commit(
                    format!("seed {path}"),
                    vec![(path.to_string(), Some(body.as_bytes().to_vec()))],
                    &author.name,
                    &author.email,
                ),
            )
            .await
            .expect("seed commit");
    }

    /// Step 1: filename matches AND frontmatter id agrees → ByFilename.
    #[tokio::test]
    async fn resolve_by_id_filename_fast_path_returns_by_filename() {
        let (backend, handle, _tmp) = test_backend().await;
        let author = test_author();
        let id = Uuid::now_v7();
        seed_two_level_memory(&backend, &handle, "rules", id, &author).await;

        let resolved = resolve_memory(&backend, &handle, None, Some(id))
            .await
            .expect("resolve");
        assert_eq!(resolved.id, id);
        assert_eq!(resolved.slug, "rules");
        assert_eq!(resolved.addressing_mode, AddressingMode::ByFilename);
    }

    /// Step 2: hand-crafted file (non-UUID filename) carries a
    /// matching frontmatter id → ByFrontmatter.
    #[tokio::test]
    async fn resolve_by_id_hand_crafted_filename_returns_by_frontmatter() {
        let (backend, handle, _tmp) = test_backend().await;
        let author = test_author();
        let id = Uuid::now_v7();
        // Hand-crafted memory: filename stem is not a UUID, but
        // frontmatter carries the canonical id.
        let body = format!(
            "+++\nid = \"{id}\"\nname = \"hand\"\ndescription = \"d\"\nkind = \"rule\"\n+++\nbody\n"
        );
        seed_raw(
            &backend,
            &handle,
            "memories/hand/scratch.md",
            &body,
            &author,
        )
        .await;

        let resolved = resolve_memory(&backend, &handle, None, Some(id))
            .await
            .expect("resolve");
        assert_eq!(resolved.id, id);
        assert_eq!(resolved.slug, "hand");
        assert_eq!(resolved.path, "memories/hand/scratch.md");
        assert_eq!(resolved.addressing_mode, AddressingMode::ByFrontmatter);
    }

    /// Step 3: UUID-named file whose stem disagrees with its
    /// frontmatter id; the resolver still finds it by frontmatter
    /// scan and returns ByFrontmatter.
    #[tokio::test]
    async fn resolve_by_id_uuid_named_mismatch_returns_by_frontmatter() {
        let (backend, handle, _tmp) = test_backend().await;
        let author = test_author();
        let filename_uuid = Uuid::now_v7();
        let frontmatter_uuid = Uuid::now_v7();
        let body = format!(
            "+++\nid = \"{frontmatter_uuid}\"\nname = \"drift\"\ndescription = \"d\"\nkind = \"rule\"\n+++\nbody\n"
        );
        let path = format!("memories/drift/{filename_uuid}.md");
        seed_raw(&backend, &handle, &path, &body, &author).await;

        // Querying by the frontmatter id resolves via step 3.
        let resolved = resolve_memory(&backend, &handle, None, Some(frontmatter_uuid))
            .await
            .expect("resolve via frontmatter");
        assert_eq!(resolved.id, frontmatter_uuid);
        assert_eq!(resolved.slug, "drift");
        assert_eq!(resolved.path, path);
        assert_eq!(resolved.addressing_mode, AddressingMode::ByFrontmatter);
    }

    /// Querying by the filename UUID of a drifted file no longer
    /// resolves: step 1 verifies frontmatter id matches, and the
    /// frontmatter id is different, so the lookup falls through
    /// every step and returns MemoryNotFound.
    #[tokio::test]
    async fn resolve_by_id_drifted_filename_uuid_is_not_found() {
        let (backend, handle, _tmp) = test_backend().await;
        let author = test_author();
        let filename_uuid = Uuid::now_v7();
        let frontmatter_uuid = Uuid::now_v7();
        let body = format!(
            "+++\nid = \"{frontmatter_uuid}\"\nname = \"d\"\ndescription = \"d\"\nkind = \"rule\"\n+++\nbody\n"
        );
        let path = format!("memories/drift/{filename_uuid}.md");
        seed_raw(&backend, &handle, &path, &body, &author).await;

        let err = resolve_memory(&backend, &handle, None, Some(filename_uuid))
            .await
            .expect_err("not found via filename uuid");
        assert!(matches!(
            err,
            ImportError::MemoryNotFound { slug: None, id: Some(i) } if i == filename_uuid
        ));
    }

    /// Step 1 wins over step 2/3: when a pristine file exists at
    /// the fast path, the resolver doesn't bother scanning
    /// hand-crafted siblings even if they'd also match.
    #[tokio::test]
    async fn resolve_by_id_prefers_filename_fast_path_over_scan() {
        let (backend, handle, _tmp) = test_backend().await;
        let author = test_author();
        let id = Uuid::now_v7();
        seed_two_level_memory(&backend, &handle, "rules", id, &author).await;
        // Drop a hand-crafted sibling that also claims `id` in its frontmatter.
        // Step 1 should still win.
        let dup_body = format!(
            "+++\nid = \"{id}\"\nname = \"dup\"\ndescription = \"d\"\nkind = \"rule\"\n+++\nbody\n"
        );
        seed_raw(
            &backend,
            &handle,
            "memories/scratch/manual.md",
            &dup_body,
            &author,
        )
        .await;

        let resolved = resolve_memory(&backend, &handle, None, Some(id))
            .await
            .expect("resolve");
        assert_eq!(resolved.slug, "rules");
        assert_eq!(resolved.addressing_mode, AddressingMode::ByFilename);
    }

    /// Candidate count large enough that a per-candidate `read_file` loop and
    /// a single `read_files` batch call are trivially distinguishable.
    const BULK_CANDIDATE_COUNT: usize = 25;

    /// [`scan_candidates_for_id`] calls its `read_batch` seam exactly once, independent of how
    /// many candidates it scans. The regression this guards is resolve_by_id's former per-candidate
    /// `read_file` loop, which called the seam once per candidate instead of once for the whole step.
    #[tokio::test]
    async fn scan_candidates_for_id_reads_the_batch_exactly_once_on_a_full_miss() {
        let expected = Uuid::now_v7();
        let candidates: Vec<(String, String)> = (0..BULK_CANDIDATE_COUNT)
            .map(|i| (format!("bulk-{i}"), format!("memories/bulk-{i}/dummy.md")))
            .collect();
        let requested_len = candidates.len();
        let call_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counted = Arc::clone(&call_count);

        let hit = scan_candidates_for_id(
            &candidates,
            expected,
            AddressingMode::ByFrontmatter,
            move |batched_paths| {
                counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                assert_eq!(
                    batched_paths.len(),
                    requested_len,
                    "every candidate must land in the single batch call"
                );
                async { Ok(BatchOutcome::new()) }
            },
        )
        .await
        .expect("an empty batch resolves to a miss, not an error");

        assert_eq!(
            call_count.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "the read seam must be called exactly once regardless of candidate count"
        );
        assert!(hit.is_none());
    }

    /// A hit partway through the candidate list still resolves from the single batch call,
    /// and the seam still fires exactly once (the fix must not retry per-candidate on a miss
    /// before the eventual hit).
    #[tokio::test]
    async fn scan_candidates_for_id_finds_a_mid_batch_hit_in_one_call() {
        let expected = Uuid::now_v7();
        let other = Uuid::now_v7();
        let candidates: Vec<(String, String)> = (0..BULK_CANDIDATE_COUNT)
            .map(|i| (format!("bulk-{i}"), format!("memories/bulk-{i}/dummy.md")))
            .collect();
        let hit_index = BULK_CANDIDATE_COUNT / 2;
        let call_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counted = Arc::clone(&call_count);

        let hit = scan_candidates_for_id(
            &candidates,
            expected,
            AddressingMode::ByFrontmatter,
            move |batched_paths| {
                counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let outcomes: BatchOutcome = batched_paths
                    .into_iter()
                    .enumerate()
                    .map(|(i, path)| {
                        let id = if i == hit_index { expected } else { other };
                        let body = format!(
                            "+++\nid = \"{id}\"\nname = \"n\"\ndescription = \"d\"\nkind = \"rule\"\n+++\nbody\n"
                        );
                        (path, Ok(bytes::Bytes::from(body.into_bytes())))
                    })
                    .collect();
                async move { Ok(outcomes) }
            },
        )
        .await
        .expect("batch read succeeds");

        assert_eq!(call_count.load(std::sync::atomic::Ordering::SeqCst), 1);
        let hit = hit.expect("mid-batch candidate must resolve");
        assert_eq!(hit.id, expected);
        assert_eq!(hit.slug, format!("bulk-{hit_index}"));
    }

    /// Slug-only lookups carry the BySlugOnly tag so write
    /// enforcement knows there was no id to compare against.
    #[tokio::test]
    async fn resolve_by_slug_returns_by_slug_only_addressing() {
        let (backend, handle, _tmp) = test_backend().await;
        let author = test_author();
        let id = Uuid::now_v7();
        seed_two_level_memory(&backend, &handle, "by-slug", id, &author).await;

        let resolved = resolve_memory(&backend, &handle, Some("by-slug"), None)
            .await
            .expect("resolve");
        assert_eq!(resolved.addressing_mode, AddressingMode::BySlugOnly);
    }

    // ── Slice D: validate_id_mismatch enforcement matrix ─────────

    fn rendered_for(id: Uuid) -> String {
        format!(
            "+++\nid = \"{id}\"\nname = \"d\"\ndescription = \"d\"\nkind = \"rule\"\n+++\nbody\n"
        )
    }

    #[test]
    fn validate_id_mismatch_match_branch() {
        let id = Uuid::now_v7();
        let path = format!("memories/rules/{id}.md");
        let rendered = rendered_for(id);
        let outcome =
            validate_id_mismatch(&path, &rendered, AddressingMode::ByFilename, false).unwrap();
        assert_eq!(outcome, IdValidation::Match);
    }

    #[test]
    fn validate_id_mismatch_by_filename_rejects_without_force() {
        let filename = Uuid::now_v7();
        let frontmatter = Uuid::now_v7();
        let path = format!("memories/rules/{filename}.md");
        let rendered = rendered_for(frontmatter);
        let err =
            validate_id_mismatch(&path, &rendered, AddressingMode::ByFilename, false).unwrap_err();
        assert!(matches!(
            err,
            ImportError::IdMismatchOnFilenameWrite { filename: f, frontmatter: g, .. }
            if f == filename && g == frontmatter
        ));
    }

    #[test]
    fn validate_id_mismatch_by_filename_with_force_returns_forced_variant() {
        let filename = Uuid::now_v7();
        let frontmatter = Uuid::now_v7();
        let path = format!("memories/rules/{filename}.md");
        let rendered = rendered_for(frontmatter);
        let outcome =
            validate_id_mismatch(&path, &rendered, AddressingMode::ByFilename, true).unwrap();
        assert!(matches!(
            outcome,
            IdValidation::MismatchForced { filename: f, frontmatter: g }
            if f == filename && g == frontmatter
        ));
    }

    #[test]
    fn validate_id_mismatch_by_frontmatter_returns_accepted_variant() {
        let filename = Uuid::now_v7();
        let frontmatter = Uuid::now_v7();
        let path = format!("memories/rules/{filename}.md");
        let rendered = rendered_for(frontmatter);
        let outcome =
            validate_id_mismatch(&path, &rendered, AddressingMode::ByFrontmatter, false).unwrap();
        assert!(matches!(
            outcome,
            IdValidation::MismatchAccepted { filename: f, frontmatter: g }
            if f == filename && g == frontmatter
        ));
    }

    #[test]
    fn validate_id_mismatch_by_slug_only_accepts_with_warning() {
        let filename = Uuid::now_v7();
        let frontmatter = Uuid::now_v7();
        let path = format!("memories/rules/{filename}.md");
        let rendered = rendered_for(frontmatter);
        let outcome =
            validate_id_mismatch(&path, &rendered, AddressingMode::BySlugOnly, false).unwrap();
        assert!(matches!(outcome, IdValidation::MismatchAccepted { .. }));
    }

    /// Hand-crafted filenames (non-UUID stem) have nothing to
    /// compare against on the filename side, so the validator
    /// returns `Match` regardless of frontmatter id.
    #[test]
    fn validate_id_mismatch_handcrafted_filename_skips_comparison() {
        let frontmatter = Uuid::now_v7();
        let rendered = rendered_for(frontmatter);
        let outcome = validate_id_mismatch(
            "memories/hand/scratch.md",
            &rendered,
            AddressingMode::ByFrontmatter,
            false,
        )
        .unwrap();
        assert_eq!(outcome, IdValidation::Match);
    }

    /// End-to-end: write_memory_by_id propagates the rejection
    /// when filename UUID disagrees with frontmatter and `force =
    /// false` under `ByFilename` addressing.
    #[tokio::test]
    async fn write_memory_by_id_rejects_filename_mismatch_without_force() {
        let (backend, handle, _tmp) = test_backend().await;
        let author = test_author();
        let filename = Uuid::now_v7();
        let frontmatter = Uuid::now_v7();
        let rendered = rendered_for(frontmatter);
        let err = write_memory_by_id(
            &backend,
            &handle,
            "rules",
            filename,
            &rendered,
            &author,
            WriteMemoryOptions {
                addressing_mode: AddressingMode::ByFilename,
                ..Default::default()
            },
        )
        .await
        .expect_err("rejection");
        assert!(matches!(err, ImportError::IdMismatchOnFilenameWrite { .. }));
    }

    /// End-to-end: write_memory_by_id with `force = true` under
    /// `ByFilename` addressing succeeds and returns the
    /// `MismatchForced` validation outcome.
    #[tokio::test]
    async fn write_memory_by_id_force_bypasses_filename_mismatch() {
        let (backend, handle, _tmp) = test_backend().await;
        let author = test_author();
        let filename = Uuid::now_v7();
        let frontmatter = Uuid::now_v7();
        let rendered = rendered_for(frontmatter);
        let (commit, validation) = write_memory_by_id(
            &backend,
            &handle,
            "rules",
            filename,
            &rendered,
            &author,
            WriteMemoryOptions {
                addressing_mode: AddressingMode::ByFilename,
                force: true,
                ..Default::default()
            },
        )
        .await
        .expect("forced write");
        assert!(!commit.is_empty());
        assert!(matches!(validation, IdValidation::MismatchForced { .. }));
    }

    fn rendered_with(name: &str, description: &str, tags: Vec<String>, body: &str) -> String {
        let file = MemoryFile {
            frontmatter: MemoryFrontmatter::new(name, description, MemoryKind::Rule)
                .with_tags(tags),
            body: body.to_string(),
            format: mmcp_core::memory::FrontmatterFormat::TomlPlus,
        };
        file.to_string().expect("render")
    }

    #[test]
    fn validate_write_content_lengths_accepts_values_at_limit() {
        let rendered = rendered_with(
            &"a".repeat(mmcp_core::memory::MAX_NAME_LENGTH),
            &"a".repeat(mmcp_core::memory::MAX_DESCRIPTION_LENGTH),
            vec!["a".repeat(mmcp_core::memory::MAX_TAG_LENGTH)],
            &"a".repeat(mmcp_core::memory::MAX_BODY_LENGTH),
        );
        assert!(validate_write_content_lengths(&rendered).is_ok());
    }

    #[test]
    fn validate_write_content_lengths_rejects_oversized_name() {
        let rendered = rendered_with(
            &"a".repeat(mmcp_core::memory::MAX_NAME_LENGTH + 1),
            "d",
            vec![],
            "body",
        );
        let err = validate_write_content_lengths(&rendered).unwrap_err();
        assert!(matches!(err, ImportError::FieldTooLong(_)));
    }

    #[test]
    fn validate_write_content_lengths_rejects_oversized_description() {
        let rendered = rendered_with(
            "n",
            &"a".repeat(mmcp_core::memory::MAX_DESCRIPTION_LENGTH + 1),
            vec![],
            "body",
        );
        let err = validate_write_content_lengths(&rendered).unwrap_err();
        assert!(matches!(err, ImportError::FieldTooLong(_)));
    }

    #[test]
    fn validate_write_content_lengths_rejects_oversized_tag() {
        let rendered = rendered_with(
            "n",
            "d",
            vec!["a".repeat(mmcp_core::memory::MAX_TAG_LENGTH + 1)],
            "body",
        );
        let err = validate_write_content_lengths(&rendered).unwrap_err();
        assert!(matches!(err, ImportError::FieldTooLong(_)));
    }

    #[test]
    fn validate_write_content_lengths_rejects_too_many_tags() {
        let too_many: Vec<String> = (0..=mmcp_core::memory::MAX_TAG_COUNT)
            .map(|i| format!("t{i}"))
            .collect();
        let rendered = rendered_with("n", "d", too_many, "body");
        let err = validate_write_content_lengths(&rendered).unwrap_err();
        assert!(matches!(err, ImportError::FieldTooLong(_)));
    }

    #[test]
    fn validate_write_content_lengths_rejects_oversized_body() {
        let rendered = rendered_with(
            "n",
            "d",
            vec![],
            &"a".repeat(mmcp_core::memory::MAX_BODY_LENGTH + 1),
        );
        let err = validate_write_content_lengths(&rendered).unwrap_err();
        assert!(matches!(err, ImportError::FieldTooLong(_)));
    }

    #[test]
    fn resolve_commit_message_accepts_override_at_limit() {
        let message = "a".repeat(mmcp_core::memory::MAX_MESSAGE_LENGTH);
        let resolved = resolve_commit_message(Some(&message), || unreachable!("override supplied"))
            .expect("at-limit message accepted");
        assert_eq!(resolved, message);
    }

    #[test]
    fn resolve_commit_message_rejects_oversized_override() {
        let message = "a".repeat(mmcp_core::memory::MAX_MESSAGE_LENGTH + 1);
        let err = resolve_commit_message(Some(&message), || unreachable!("override supplied"))
            .unwrap_err();
        assert!(matches!(err, ImportError::FieldTooLong(_)));
    }

    #[test]
    fn resolve_commit_message_uses_fallback_when_absent() {
        let resolved =
            resolve_commit_message(None, || "synthesized".to_string()).expect("fallback used");
        assert_eq!(resolved, "synthesized");
    }

    /// End-to-end: `import_memory` (the primitive backing the MCP
    /// `write_memory` tool) rejects an oversized body through the
    /// real write path, proving the check is actually wired at the
    /// public entry point and not just on the private helper.
    #[tokio::test]
    async fn import_memory_rejects_oversized_body_end_to_end() {
        let (backend, handle, _tmp) = test_backend().await;
        let author = test_author();
        let content = rendered_with(
            "n",
            "d",
            vec![],
            &"a".repeat(mmcp_core::memory::MAX_BODY_LENGTH + 1),
        );
        let err = import_memory(
            &backend,
            &handle,
            "oversized",
            &content,
            None,
            &author,
            false,
        )
        .await
        .unwrap_err();
        assert!(matches!(err, ImportError::FieldTooLong(_)));
    }

    /// Same end-to-end check on the accept side: a body exactly at
    /// the limit is written successfully.
    #[tokio::test]
    async fn import_memory_accepts_body_at_limit_end_to_end() {
        let (backend, handle, _tmp) = test_backend().await;
        let author = test_author();
        let content = rendered_with(
            "n",
            "d",
            vec![],
            &"a".repeat(mmcp_core::memory::MAX_BODY_LENGTH),
        );
        let result = import_memory(
            &backend, &handle, "at-limit", &content, None, &author, false,
        )
        .await
        .expect("write at the limit succeeds");
        assert!(!result.commit_id.is_empty());
    }
}
