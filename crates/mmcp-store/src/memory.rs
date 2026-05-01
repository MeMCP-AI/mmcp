//! Memory CRUD primitives plus the `import_memory` upsert wrapper.
//!
//! Three typed primitives (`create_memory_file`, `update_memory_file`,
//! `delete_memory_file`) wrap `NativeBackend::write_commit` with
//! existence checks so every consumer — the CLI, the MCP tools, the
//! GUI, or any third-party caller — enforces a strict contract
//! without re-implementing the probe. Each primitive maps a missing
//! or collision slug to a structured [`ImportError`] variant.
//!
//! `import_memory` is the higher-level wrapper used by the CLI
//! `mmcp import` path and by the MCP `write_memory` tool: it parses
//! or synthesises frontmatter, chooses between create and update
//! based on the caller-supplied `override_existing` flag, and
//! commits.
//!
//! History: ported from `crates/mmcp-client/src/commands/import.rs`
//! during the FR-020 extraction. `ImportError` keeps its historical
//! name even though the module now covers broader CRUD concerns; a
//! rename to `MemoryError` is a cosmetic follow-up that would
//! ripple across every consumer's error mapper, not worth it for
//! this chain.

use mmcp_core::id::GroupId;
use mmcp_core::memory::{MemoryFile, MemoryFrontmatter, MemoryKind};
use mmcp_git::{CommitSpec, GitBackend, GitError, NativeBackend, RepoHandle, Rev};
use uuid::Uuid;

use crate::groups::{GroupEntry, GroupIndex};
use crate::home::ResolvedAuthor;

/// Result of a successful import.
#[derive(Debug, Clone)]
pub struct ImportResult {
    pub slug: String,
    /// Canonical UUID minted for this memory (or taken from the
    /// source's frontmatter when it carried one). Callers use this
    /// to address the memory across the two-level
    /// `memories/<slug>/<uuid>.md` layout without re-resolving by
    /// slug, which is ambiguous once siblings exist.
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
/// Named `ImportError` for historical reasons (this module started
/// out as the `mmcp import` implementation); it now covers the
/// shared create/update/delete primitives too. A rename is a
/// cosmetic follow-up.
#[derive(Debug, thiserror::Error)]
pub enum ImportError {
    #[error(
        "invalid slug '{0}': must be 1-128 chars, lowercase alphanumeric with hyphens, no leading/trailing hyphens"
    )]
    InvalidSlug(String),

    #[error("content has no +++ frontmatter and no synthetic frontmatter provided")]
    MissingFrontmatter,

    #[error("frontmatter parse error: {0}")]
    Parse(#[from] mmcp_core::memory::MemoryParseError),

    #[error("git error: {0}")]
    Git(#[from] mmcp_git::GitError),

    #[error("render error: {0}")]
    Render(String),

    #[error("unknown memory kind '{0}': expected rule, snapshot, log, reference, or scratch")]
    UnknownKind(String),

    #[error("group not found: {0}")]
    GroupNotFound(String),

    /// A `create` was attempted against a slug that is already on
    /// disk. The caller picks between `update` (edit-in-place) and
    /// `create` with an explicit override to replace.
    #[error("memory '{slug}' already exists in this group")]
    MemoryAlreadyExists { slug: String },

    /// An `update` or `delete` was attempted against a slug that
    /// has no file in the group. Distinct from `GroupNotFound`,
    /// which signals a missing group altogether. Post-FR-028 the
    /// lookup may have been keyed on either `slug`, `id`, or both,
    /// so both fields are optional; callers populate whichever
    /// addresses they actually tried.
    #[error("memory not found (slug={slug:?}, id={id:?})")]
    MemoryNotFound {
        slug: Option<String>,
        id: Option<Uuid>,
    },

    /// A slug-only lookup resolved to more than one memory under
    /// `memories/<slug>/`. The caller must re-query with an
    /// explicit `id` from the candidate list.
    #[error("memory slug '{slug}' has multiple entries; disambiguate with id")]
    MemoryAmbiguous {
        slug: String,
        candidates: Vec<Uuid>,
    },

    /// Both `slug` and `id` were supplied but the on-disk memory's
    /// frontmatter carries a different id. Signals either a stale
    /// client cache or a corrupted frontmatter pair.
    #[error("memory '{slug}' id mismatch: expected {expected}, got {got}")]
    MemoryIdMismatch {
        slug: String,
        expected: Uuid,
        got: Uuid,
    },

    /// A write addressed by filename UUID (the caller specified the
    /// filename path explicitly, or resolved via the filename fast
    /// path) carried a frontmatter `id` that disagrees with the
    /// filename. Slice D rejects this by default; callers that
    /// genuinely intend to overwrite a drifted file pass `force =
    /// true` to flip the rejection into an accepted-with-note path.
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
}

/// Per-file reference to a memory on disk. Returned by
/// [`list_all_memory_files`] so callers get a direct path plus
/// the slug/id pair the `memories/<slug>/<uuid>.md` layout
/// encodes.
#[derive(Debug, Clone)]
pub struct MemoryFileRef {
    pub slug: String,
    pub id: Uuid,
    pub path: String,
}

/// Walk every memory file in the group at `rev`. Each slug
/// subdirectory under `memories/` is enumerated and every
/// UUID-named `.md` file inside surfaces as one entry; duplicate
/// slugs appear as multiple entries with distinct UUIDs.
///
/// Used by diagnostics and any other consumer that needs to read
/// every memory exactly once.
pub async fn list_all_memory_files(
    backend: &NativeBackend,
    handle: &RepoHandle,
    rev: &Rev,
) -> Result<Vec<MemoryFileRef>, GitError> {
    let dir = mmcp_core::conventions::MEMORIES_DIR;
    let ext = mmcp_core::conventions::MEMORY_EXTENSION;
    let mut out = Vec::new();
    for slug in backend.list_subtrees(handle, dir, rev).await? {
        let subdir = format!("{dir}/{slug}");
        for filename in backend.list_tree(handle, &subdir, rev).await? {
            let Some(stem) = filename.strip_suffix(ext) else {
                continue;
            };
            let Ok(id) = Uuid::parse_str(stem) else {
                // Ignore stray non-UUID files; the layout contract
                // says every memory filename is a UUIDv7.
                continue;
            };
            out.push(MemoryFileRef {
                slug: slug.clone(),
                id,
                path: format!("{subdir}/{filename}"),
            });
        }
    }
    Ok(out)
}

/// How a [`ResolvedMemory`] was reached. Branches the write
/// enforcement rules in Slice D (FR-28): filename-addressed writes
/// reject on id mismatch unless `force`, frontmatter-addressed
/// writes accept with a `malformed_frontmatter` warning note, and
/// slug-only queries skip the mismatch check because no id was
/// provided to compare against.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AddressingMode {
    /// Reached via the filename fast path — file at
    /// `memories/<slug>/<id>.md` exists AND its frontmatter id
    /// matches the queried id. Writes in this mode treat the
    /// filename UUID as authoritative and surface mismatches as
    /// hard rejections (unless the caller passes `force: true`).
    ByFilename,
    /// Reached by scanning frontmatter ids across the group after
    /// the filename fast path missed. Either the file was
    /// hand-crafted with a non-UUID filename, or its filename
    /// UUID disagrees with the stored frontmatter id. Writes in
    /// this mode accept the edit and emit a
    /// `malformed_frontmatter` warning so the drift stays visible.
    ByFrontmatter,
    /// Reached via slug-only resolution; no id was supplied, so
    /// there is no filename/frontmatter comparison to make. Writes
    /// branch through this mode the same way they always have.
    BySlugOnly,
}


/// Outcome of the filename-vs-frontmatter id check that
/// [`validate_id_mismatch`] runs on every write. Callers map this
/// onto FR-45 notes (`id_mismatch_accepted` / `id_mismatch_forced`)
/// at the tool boundary.
///
/// `Match` is the silent common case. The two mismatch variants
/// distinguish acceptance paths: `MismatchAccepted` rides on
/// frontmatter-as-truth (D4b/D4c), `MismatchForced` rides on
/// caller-asserted override of the filename addressing rule (D4a
/// + force).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdValidation {
    /// Filename UUID and frontmatter id agree, or one of them was
    /// absent (no comparison possible).
    Match,
    /// Filename ≠ frontmatter; the write was addressed by
    /// frontmatter / slug only, so frontmatter is source of truth
    /// and the write proceeds. Surface as a `id_mismatch_accepted`
    /// note.
    MismatchAccepted { filename: Uuid, frontmatter: Uuid },
    /// Filename ≠ frontmatter; the write was addressed by filename
    /// UUID and the caller passed `force = true` to override the
    /// rejection rule. Surface as a `id_mismatch_forced` note.
    MismatchForced { filename: Uuid, frontmatter: Uuid },
}

/// Compare the filename UUID encoded in `path` against the
/// frontmatter `id` stamped in `rendered`. Apply the D4 enforcement
/// rules and return the resulting [`IdValidation`].
///
/// Pure function (no I/O). Callers commit only after the validation
/// resolves to a non-error outcome; the FR-45 note emitted from the
/// returned variant tags the response so consumers see the drift.
pub fn validate_id_mismatch(
    path: &str,
    rendered: &str,
    addressing_mode: AddressingMode,
    force: bool,
) -> Result<IdValidation, ImportError> {
    // Extract filename UUID from path stem `<uuid>.md`. If the
    // path doesn't end in a UUID stem (hand-crafted slugs), there
    // is nothing to compare; treat as Match.
    let filename = filename_uuid_from_path(path);
    let frontmatter = parse_frontmatter_id(rendered.as_bytes());
    let (filename, frontmatter) = match (filename, frontmatter) {
        (Some(f), Some(g)) => (f, g),
        // Either side absent means no comparison applies. Diagnose
        // separately flags missing frontmatter ids; the resolver
        // already requires one for `ByFrontmatter` resolution.
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

/// Extract the trailing `<uuid>.md` stem from a `memories/<slug>/<uuid>.md`
/// path. Returns `None` for hand-crafted filenames whose stem is
/// not a UUID.
fn filename_uuid_from_path(path: &str) -> Option<Uuid> {
    let stem = path
        .rsplit('/')
        .next()
        .and_then(|name| name.strip_suffix(mmcp_core::conventions::MEMORY_EXTENSION))?;
    Uuid::parse_str(stem).ok()
}

/// Addressing result from [`resolve_memory`]. Carries the slug,
/// the canonical UUID, and the in-repo path
/// (`memories/<slug>/<uuid>.md`) that a subsequent `read_file` can
/// consume verbatim.
#[derive(Debug, Clone)]
pub struct ResolvedMemory {
    pub slug: String,
    pub id: Uuid,
    pub path: String,
    /// How the resolver located this entry. Callers that write
    /// branch on this to decide whether a filename/frontmatter id
    /// mismatch is a hard reject or a soft warning.
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
    let path = mmcp_core::conventions::memory_path(slug, expected);
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
    let dir = format!(
        "{}/{}",
        mmcp_core::conventions::MEMORIES_DIR,
        slug
    );
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
                path: mmcp_core::conventions::memory_path(slug, only),
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
    // Walk every slug directory once, splitting files into the
    // three buckets the fallback chain works through (D6):
    //   1. UUID-named files whose stem == `expected`        (step 1 candidates)
    //   2. non-UUID-named files (hand-crafted slugs)         (step 2 candidates)
    //   3. UUID-named files whose stem != `expected`         (step 3 candidates)
    // Every file read goes through `parse_frontmatter_id` so the
    // frontmatter id is the source of truth (FR-28 / D4).
    let rev = Rev::head();
    let dirs = backend
        .list_subtrees(handle, mmcp_core::conventions::MEMORIES_DIR, &rev)
        .await?;

    let filename_ext = mmcp_core::conventions::MEMORY_EXTENSION;
    let mut step1_candidates: Vec<(String, String)> = Vec::new();
    let mut step2_candidates: Vec<(String, String)> = Vec::new();
    let mut step3_candidates: Vec<(String, String)> = Vec::new();

    for slug in &dirs {
        let dir = format!("{}/{}", mmcp_core::conventions::MEMORIES_DIR, slug);
        let entries = backend.list_tree(handle, &dir, &rev).await?;
        for name in entries {
            let Some(stem) = name.strip_suffix(filename_ext) else {
                // Non-`.md` files are a schema violation that
                // `diagnose` already flags; the resolver ignores
                // them so a stray `.DS_Store` doesn't poison the
                // fallback scan.
                continue;
            };
            let path = format!("{}/{}/{}", mmcp_core::conventions::MEMORIES_DIR, slug, name);
            match Uuid::parse_str(stem) {
                Ok(file_uuid) if file_uuid == expected => {
                    step1_candidates.push((slug.clone(), path));
                }
                Ok(_) => {
                    step3_candidates.push((slug.clone(), path));
                }
                Err(_) => {
                    step2_candidates.push((slug.clone(), path));
                }
            }
        }
    }

    // Step 1: filename fast path. Stem already matches `expected`;
    // verify the frontmatter id agrees before declaring a hit.
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

    // Step 2: hand-crafted memories (non-UUID filenames). Parse
    // frontmatter and match on its id. Reached only when step 1
    // missed because most repos have no non-UUID files.
    for (slug, path) in &step2_candidates {
        let Ok(bytes) = backend.read_file(handle, path, &rev).await else {
            continue;
        };
        if parse_frontmatter_id(&bytes) == Some(expected) {
            return Ok(ResolvedMemory {
                slug: slug.clone(),
                id: expected,
                path: path.clone(),
                addressing_mode: AddressingMode::ByFrontmatter,
            });
        }
    }

    // Step 3: UUID-named files whose filename stem disagrees with
    // `expected`. Their frontmatter may still match the queried
    // id — a drift the resolver honours (frontmatter is source of
    // truth) while leaving the addressing mode as `ByFrontmatter`
    // so writes route through the soft-warning branch.
    for (slug, path) in &step3_candidates {
        let Ok(bytes) = backend.read_file(handle, path, &rev).await else {
            continue;
        };
        if parse_frontmatter_id(&bytes) == Some(expected) {
            return Ok(ResolvedMemory {
                slug: slug.clone(),
                id: expected,
                path: path.clone(),
                addressing_mode: AddressingMode::ByFrontmatter,
            });
        }
    }

    Err(ImportError::MemoryNotFound {
        slug: None,
        id: Some(expected),
    })
}

/// Result entry from [`read_frontmatters_in_group`]. Carries the
/// file reference (slug, id, repo path) alongside the frontmatter
/// parse outcome so a single corrupt file does not abort the whole
/// listing — the caller can choose to ignore, log, or propagate
/// per-entry errors.
#[derive(Debug)]
pub struct MemoryFrontmatterEntry {
    pub file: MemoryFileRef,
    pub frontmatter: Result<MemoryFrontmatter, ImportError>,
}

/// Read just the frontmatter of a memory by slug and/or id.
///
/// Resolves the memory via [`resolve_memory`] and parses the
/// on-disk file via [`MemoryFile::parse`], discarding the body
/// before return. Use this when the caller only needs frontmatter
/// fields (kind, name, description, tags, mandatory) and has many
/// memories to scan — e.g. a GUI memory-list panel rendering kind
/// prefixes for every slug, or a feature summariser collapsing
/// records onto a metadata-only wire shape. Single reads where
/// the body is also needed should keep using the full read path.
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
/// Wraps [`list_all_memory_files`] + per-file frontmatter parse
/// into one fan-out call. Per-file errors land inside each entry's
/// `frontmatter` field rather than aborting the iteration, so one
/// malformed file in a 100-memory group does not blank the whole
/// listing. Top-level git errors (the directory walk itself) still
/// surface as `Err`.
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

async fn read_frontmatter_at(
    backend: &NativeBackend,
    handle: &RepoHandle,
    rev: &Rev,
    path: &str,
) -> Result<MemoryFrontmatter, ImportError> {
    let bytes = backend.read_file(handle, path, rev).await?;
    let text = String::from_utf8_lossy(&bytes).into_owned();
    let file = MemoryFile::parse(&text)?;
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

/// Commit a write of `rendered` at an explicit repo-relative
/// `path` after running the FR-28 / D4 id-mismatch check. The
/// validation compares the filename UUID encoded in `path` to the
/// frontmatter `id` in `rendered` and applies the rules from
/// [`validate_id_mismatch`]. Callers thread the
/// `addressing_mode` from their resolver and `force` from their
/// tool args.
///
/// On success returns the commit id and the [`IdValidation`]
/// outcome so the caller can surface `id_mismatch_*` FR-45 notes.
/// `IdMismatchOnFilenameWrite` short-circuits before writing.
pub async fn write_file_at_path(
    backend: &NativeBackend,
    handle: &RepoHandle,
    path: &str,
    rendered: &str,
    author: &ResolvedAuthor,
    addressing_mode: AddressingMode,
    force: bool,
    message: Option<&str>,
) -> Result<(String, IdValidation), ImportError> {
    let validation = validate_id_mismatch(path, rendered, addressing_mode, force)?;
    let commit_message = message
        .map(str::to_string)
        .unwrap_or_else(|| format!("write {path}"));
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
    Ok((commit_id, validation))
}

/// Commit a deletion of `path`. Unconditional; callers probe first
/// if they want a "not found" error.
pub async fn delete_file_at_path(
    backend: &NativeBackend,
    handle: &RepoHandle,
    path: &str,
    author: &ResolvedAuthor,
    message: Option<&str>,
) -> Result<String, ImportError> {
    let commit_message = message
        .map(str::to_string)
        .unwrap_or_else(|| format!("delete {path}"));
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

/// Write a memory at the two-level `memories/<slug>/<id>.md` path
/// with create-or-override semantics. Delegates the FR-28 / D4
/// id-mismatch check to [`write_file_at_path`] so callers thread
/// `addressing_mode` and `force` through both primitives.
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
    override_existing: bool,
    addressing_mode: AddressingMode,
    force: bool,
    message: Option<&str>,
) -> Result<(String, IdValidation), ImportError> {
    validate_slug(slug)?;
    let path = mmcp_core::conventions::memory_path(slug, id);
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
        addressing_mode,
        force,
        Some(&commit_message),
    )
    .await
}

/// Create a fresh memory file. Errors with
/// Import a memory into a group repo.
///
/// Parses `content` as a full memory file (frontmatter + body);
/// when the content lacks a `+++` block, `synth_frontmatter` must
/// supply name / description / kind and the rest of the body is
/// treated as the payload. The memory lands at
/// `memories/<slug>/<uuid>.md` — the id is taken from frontmatter
/// when present, otherwise a fresh UUIDv7 is minted.
///
/// `override_existing` only matters when an explicit id from
/// frontmatter collides with an existing file. With a freshly
/// minted id, the write always creates a new sibling under the
/// slug directory (duplicate slugs are legal post-FR-028).
pub async fn import_memory(
    backend: &NativeBackend,
    handle: &RepoHandle,
    slug: &str,
    content: &str,
    synth_frontmatter: Option<SynthFrontmatter>,
    author: &ResolvedAuthor,
    override_existing: bool,
) -> Result<ImportResult, ImportError> {
    validate_slug(slug)?;

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

    // FR-39 v2: import_memory creates (or replaces under
    // override) one memory under `memories/<slug>/<id>.md`. Take
    // the create chain (Process-Shared + Group-Exclusive) so
    // concurrent imports against the same group serialise on UUID
    // minting and file creation regardless of kind. The shared
    // ticket counter and slug-uniqueness invariant both rely on
    // the group-wide exclusive view.
    let _guards =
        crate::lock::acquire_chain(&crate::lock::create_chain(handle.group_id)).await;

    let message = format!("import memory {slug}/{id}");
    // FR-28 / D4: import_memory mints `id` and stamps it into
    // frontmatter on the line above, so filename and frontmatter
    // agree by construction. Use `BySlugOnly` (the import flow has
    // no caller-supplied addressing) and `force=false`; the
    // mismatch check is a no-op here.
    let (commit_id, _validation) = write_memory_by_id(
        backend,
        handle,
        slug,
        id,
        &rendered,
        author,
        override_existing,
        AddressingMode::BySlugOnly,
        false,
        Some(&message),
    )
    .await?;

    Ok(ImportResult {
        slug: slug.to_string(),
        id,
        commit_id,
    })
}

/// Validate a memory slug.
pub fn validate_slug(slug: &str) -> Result<(), ImportError> {
    if slug.is_empty() || slug.len() > 128 {
        return Err(ImportError::InvalidSlug(slug.to_string()));
    }
    if slug.starts_with('-') || slug.ends_with('-') {
        return Err(ImportError::InvalidSlug(slug.to_string()));
    }
    for ch in slug.chars() {
        if !ch.is_ascii_lowercase() && !ch.is_ascii_digit() && ch != '-' {
            return Err(ImportError::InvalidSlug(slug.to_string()));
        }
    }
    if slug.contains("--") {
        return Err(ImportError::InvalidSlug(slug.to_string()));
    }
    Ok(())
}

/// Derive a slug from a filename.
///
/// Strips the known import extensions (`.md`, plus each entry in
/// [`crate::import_adoc::ADOC_EXTENSIONS`]) case-insensitively before
/// delegating to the `slug` crate, which handles Unicode
/// normalization (NFD + diacritic stripping) and hyphen collapsing.
/// The extra adoc / asciidoc cases exist so an operator importing
/// `coding-rules.adoc` gets the `coding-rules` slug rather than
/// `coding-rules-adoc`; the on-disk memory still lands as `.md`.
pub fn slugify_filename(filename: &str) -> String {
    let stem = strip_known_import_extension(filename);
    slug::slugify(stem)
}

/// Return `filename` with its trailing `.md` / `.adoc` / `.asciidoc`
/// extension stripped, if any. Case-insensitive on the extension so
/// `README.MD` and `Notes.ADOC` slim down the same as their lower-case
/// siblings. Returns the input unchanged when no known extension
/// matches.
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

/// Parse a kind string into `MemoryKind`.
pub fn parse_kind(s: &str) -> Result<MemoryKind, ImportError> {
    match s {
        "rule" => Ok(MemoryKind::Rule),
        "snapshot" => Ok(MemoryKind::Snapshot),
        "log" => Ok(MemoryKind::Log),
        "reference" => Ok(MemoryKind::Reference),
        "scratch" => Ok(MemoryKind::Scratch),
        other => Err(ImportError::UnknownKind(other.to_string())),
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
        let owner = Uuid::now_v7();
        let group_id = GroupId::new();
        let manifest = GroupManifest::new_user_owned(group_id, "test", owner);
        let handle = backend.create_group_repo(&manifest).await.expect("create");
        (backend, handle, tmp)
    }

    #[test]
    fn validate_slug_accepts_valid() {
        assert!(validate_slug("hello").is_ok());
        assert!(validate_slug("hello-world").is_ok());
        assert!(validate_slug("a").is_ok());
        assert!(validate_slug("foo-bar-baz-123").is_ok());
    }

    #[test]
    fn validate_slug_rejects_invalid() {
        assert!(validate_slug("").is_err());
        assert!(validate_slug("-leading").is_err());
        assert!(validate_slug("trailing-").is_err());
        assert!(validate_slug("UPPER").is_err());
        assert!(validate_slug("has space").is_err());
        assert!(validate_slug("double--hyphen").is_err());
        assert!(validate_slug(&"a".repeat(129)).is_err());
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
        // The import bridge converts adoc sources to markdown before
        // storage, but the slug is still derived from the original
        // file name. Without this strip, an operator importing
        // `coding-rules.adoc` would end up with the `coding-rules-adoc`
        // slug, which carries the source format into a field that
        // should only reflect the memory's identity.
        assert_eq!(slugify_filename("coding-rules.adoc"), "coding-rules");
        assert_eq!(slugify_filename("CODING-RULES.ADOC"), "coding-rules");
        assert_eq!(slugify_filename("team/guide.asciidoc"), "team-guide");
        assert_eq!(slugify_filename("Team Guide.AsciiDoc"), "team-guide");
    }

    #[test]
    fn parse_kind_round_trips() {
        assert_eq!(parse_kind("rule").unwrap(), MemoryKind::Rule);
        assert_eq!(parse_kind("reference").unwrap(), MemoryKind::Reference);
        assert!(parse_kind("bogus").is_err());
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
        // Resolving the slug finds exactly one entry — the one
        // we just minted — and its path sits under the slug dir.
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
        // Duplicate slugs are legal post-FR-028: each import
        // mints a fresh UUIDv7 and lands as a sibling of the
        // prior file under the same slug directory.
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
                        mmcp_core::conventions::memory_path(slug, id),
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

        // Caller supplies `other`; the two-level path `memories/mm/<other>.md`
        // doesn't exist, so the resolver falls back to the legacy flat
        // path `memories/mm.md`. That also doesn't exist here, so the
        // error should be MemoryNotFound with both keys populated.
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
        // Seed a YAML-fenced memory directly so the parser's `---`
        // branch is exercised end-to-end. FR-006 (universal frontmatter)
        // means YAML must round-trip through the same primitive.
        let yaml_body = format!(
            "---\nid: \"{id}\"\nname: yam\ndescription: yaml fenced\nkind: rule\n---\nBody after yaml fence.\n"
        );
        backend
            .write_commit(
                &handle,
                CommitSpec::mmcp_commit(
                    format!("seed yam/{id}"),
                    vec![(
                        mmcp_core::conventions::memory_path("yam", id),
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
                        mmcp_core::conventions::memory_path("bad", bad_id),
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
            .read_file(
                &handle,
                &resolved.path,
                &mmcp_git::Rev::Branch("main".to_string()),
            )
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

    /// Seed an arbitrary file at an arbitrary path. Used by the
    /// addressing-mode tests to construct hand-crafted layouts the
    /// regular `import_memory` path won't produce on its own.
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
        seed_raw(&backend, &handle, "memories/hand/scratch.md", &body, &author).await;

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
        // Drop a hand-crafted sibling that also claims `id` in its
        // frontmatter. Step 1 should still win.
        let dup_body = format!(
            "+++\nid = \"{id}\"\nname = \"dup\"\ndescription = \"d\"\nkind = \"rule\"\n+++\nbody\n"
        );
        seed_raw(&backend, &handle, "memories/scratch/manual.md", &dup_body, &author).await;

        let resolved = resolve_memory(&backend, &handle, None, Some(id))
            .await
            .expect("resolve");
        assert_eq!(resolved.slug, "rules");
        assert_eq!(resolved.addressing_mode, AddressingMode::ByFilename);
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
        assert!(matches!(
            outcome,
            IdValidation::MismatchAccepted { .. }
        ));
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
            false,
            AddressingMode::ByFilename,
            false,
            None,
        )
        .await
        .expect_err("rejection");
        assert!(matches!(
            err,
            ImportError::IdMismatchOnFilenameWrite { .. }
        ));
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
            false,
            AddressingMode::ByFilename,
            true,
            None,
        )
        .await
        .expect("forced write");
        assert!(!commit.is_empty());
        assert!(matches!(
            validation,
            IdValidation::MismatchForced { .. }
        ));
    }
}
