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

    /// Neither `slug` nor `id` was provided to a resolver call that
    /// requires at least one addressing key.
    #[error("resolve_memory requires at least one of slug or id")]
    ResolveArgsMissing,
}

/// Probe whether `memories/<slug>.md` exists at the group's
/// current `main` head.
///
/// Returns `Ok(false)` on `PathNotFound` — the "does not exist"
/// case is not an error condition. Any other git failure
/// propagates as `ImportError::Git` so callers don't confuse
/// transport / corruption errors with a missing file.
pub async fn memory_exists(
    backend: &NativeBackend,
    handle: &RepoHandle,
    slug: &str,
) -> Result<bool, ImportError> {
    let path = mmcp_core::conventions::legacy_memory_path(slug);
    match backend.read_file(handle, &path, &Rev::head()).await {
        Ok(_) => Ok(true),
        Err(GitError::PathNotFound(_)) => Ok(false),
        Err(err) => Err(ImportError::Git(err)),
    }
}

/// Addressing result from [`resolve_memory`]. Carries the slug,
/// the canonical UUID (minted for pre-FR-028 files that still
/// lack one in frontmatter), and the in-repo path that a subsequent
/// `read_file` can consume verbatim.
#[derive(Debug, Clone)]
pub struct ResolvedMemory {
    pub slug: String,
    /// May be `None` when the on-disk file is still in the legacy
    /// `memories/<slug>.md` layout and its frontmatter has no `id`.
    /// Resolvers never synthesize an id on the fly; the migration
    /// binary is the only path that mints UUIDs for legacy files.
    pub id: Option<Uuid>,
    pub path: String,
}

/// Locate a memory by slug, id, or both.
///
/// Lookup rules (mirrors the FR-028 plan):
/// - `slug + id`: address `memories/<slug>/<id>.md` first; if the
///   file exists and its frontmatter id matches, return it. If the
///   frontmatter id disagrees, surface [`ImportError::MemoryIdMismatch`].
///   Fall back to `memories/<slug>.md` (legacy layout) when the
///   two-level path is missing; require frontmatter id match there
///   too, or return [`ImportError::MemoryNotFound`].
/// - `slug` only: walk `memories/<slug>/` and use the single entry
///   found; ≥2 entries yields [`ImportError::MemoryAmbiguous`]; 0
///   entries falls back to the flat `memories/<slug>.md` legacy
///   path; missing both is [`ImportError::MemoryNotFound`].
/// - `id` only: enumerate `memories/*/` and pick the single
///   directory whose listing contains `<id>.md`.
/// - neither: [`ImportError::ResolveArgsMissing`].
///
/// The helper does not read the memory body; it only resolves the
/// logical address to a filesystem path so callers can proceed with
/// `read_file` / `write_commit` on a known key.
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
    let two_level = mmcp_core::conventions::memory_path(slug, expected);
    match backend.read_file(handle, &two_level, &Rev::head()).await {
        Ok(bytes) => {
            verify_id_match(slug, &bytes, expected)?;
            Ok(ResolvedMemory {
                slug: slug.to_string(),
                id: Some(expected),
                path: two_level,
            })
        }
        Err(GitError::PathNotFound(_)) => {
            // Legacy fallback: the file predates FR-028 and still
            // lives at `memories/<slug>.md`. We still require the
            // frontmatter id to match when it's present so stale
            // client caches never masquerade as fresh hits.
            let legacy = mmcp_core::conventions::legacy_memory_path(slug);
            match backend.read_file(handle, &legacy, &Rev::head()).await {
                Ok(bytes) => {
                    verify_id_match(slug, &bytes, expected)?;
                    Ok(ResolvedMemory {
                        slug: slug.to_string(),
                        id: Some(expected),
                        path: legacy,
                    })
                }
                Err(GitError::PathNotFound(_)) => Err(ImportError::MemoryNotFound {
                    slug: Some(slug.to_string()),
                    id: Some(expected),
                }),
                Err(err) => Err(ImportError::Git(err)),
            }
        }
        Err(err) => Err(ImportError::Git(err)),
    }
}

async fn resolve_by_slug(
    backend: &NativeBackend,
    handle: &RepoHandle,
    slug: &str,
) -> Result<ResolvedMemory, ImportError> {
    // Two-level layout first. `list_tree` on `memories/<slug>`
    // returns only the direct UUID-named blobs; subtrees would be
    // a schema violation here.
    let two_level_dir = format!(
        "{}/{}",
        mmcp_core::conventions::MEMORIES_DIR,
        slug
    );
    let entries = backend
        .list_tree(handle, &two_level_dir, &Rev::head())
        .await?;
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
                id: Some(only),
                path: mmcp_core::conventions::memory_path(slug, only),
            })
        }
        n if n >= 2 => Err(ImportError::MemoryAmbiguous {
            slug: slug.to_string(),
            candidates: uuids,
        }),
        _ => {
            // Fall back to the flat legacy layout.
            let legacy = mmcp_core::conventions::legacy_memory_path(slug);
            match backend.read_file(handle, &legacy, &Rev::head()).await {
                Ok(bytes) => {
                    let id_from_file = parse_frontmatter_id(&bytes);
                    Ok(ResolvedMemory {
                        slug: slug.to_string(),
                        id: id_from_file,
                        path: legacy,
                    })
                }
                Err(GitError::PathNotFound(_)) => Err(ImportError::MemoryNotFound {
                    slug: Some(slug.to_string()),
                    id: None,
                }),
                Err(err) => Err(ImportError::Git(err)),
            }
        }
    }
}

async fn resolve_by_id(
    backend: &NativeBackend,
    handle: &RepoHandle,
    expected: Uuid,
) -> Result<ResolvedMemory, ImportError> {
    let filename = format!(
        "{}{}",
        expected,
        mmcp_core::conventions::MEMORY_EXTENSION
    );
    let dirs = backend
        .list_subtrees(handle, mmcp_core::conventions::MEMORIES_DIR, &Rev::head())
        .await?;
    for slug in dirs {
        let dir = format!("{}/{}", mmcp_core::conventions::MEMORIES_DIR, slug);
        let entries = backend.list_tree(handle, &dir, &Rev::head()).await?;
        if entries.iter().any(|name| name == &filename) {
            return Ok(ResolvedMemory {
                slug: slug.clone(),
                id: Some(expected),
                path: mmcp_core::conventions::memory_path(&slug, expected),
            });
        }
    }
    Err(ImportError::MemoryNotFound {
        slug: None,
        id: Some(expected),
    })
}

fn verify_id_match(slug: &str, bytes: &[u8], expected: Uuid) -> Result<(), ImportError> {
    let Some(actual) = parse_frontmatter_id(bytes) else {
        // Pre-FR-028 file with no frontmatter id. Treat as a match
        // so migration flows can address by expected id without
        // erroring; the migration binary is what stamps the id.
        return Ok(());
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

/// Create a fresh memory file. Errors with
/// [`ImportError::MemoryAlreadyExists`] when the slug is already
/// on disk so callers never silently overwrite.
pub async fn create_memory_file(
    backend: &NativeBackend,
    handle: &RepoHandle,
    slug: &str,
    rendered: &str,
    author: &ResolvedAuthor,
    message: Option<&str>,
) -> Result<String, ImportError> {
    validate_slug(slug)?;
    if memory_exists(backend, handle, slug).await? {
        return Err(ImportError::MemoryAlreadyExists {
            slug: slug.to_string(),
        });
    }
    let commit_message = message
        .map(str::to_string)
        .unwrap_or_else(|| format!("create memory {slug}"));
    write_memory_commit(backend, handle, slug, rendered, author, commit_message).await
}

/// Update an existing memory file. Errors with
/// [`ImportError::MemoryNotFound`] when the slug has no file, so a
/// caller that forgot a `create_memory_file` call never silently
/// creates a new memory here.
pub async fn update_memory_file(
    backend: &NativeBackend,
    handle: &RepoHandle,
    slug: &str,
    rendered: &str,
    author: &ResolvedAuthor,
    message: Option<&str>,
) -> Result<String, ImportError> {
    validate_slug(slug)?;
    if !memory_exists(backend, handle, slug).await? {
        return Err(ImportError::MemoryNotFound {
            slug: Some(slug.to_string()),
            id: None,
        });
    }
    let commit_message = message
        .map(str::to_string)
        .unwrap_or_else(|| format!("update memory {slug}"));
    write_memory_commit(backend, handle, slug, rendered, author, commit_message).await
}

/// Commit a deletion of `memories/<slug>.md`. Errors with
/// [`ImportError::MemoryNotFound`] when the slug has no file, to
/// keep the wire contract symmetric with `update_memory_file`
/// rather than silently no-op'ing.
pub async fn delete_memory_file(
    backend: &NativeBackend,
    handle: &RepoHandle,
    slug: &str,
    author: &ResolvedAuthor,
    message: Option<&str>,
) -> Result<String, ImportError> {
    validate_slug(slug)?;
    if !memory_exists(backend, handle, slug).await? {
        return Err(ImportError::MemoryNotFound {
            slug: Some(slug.to_string()),
            id: None,
        });
    }
    let commit_message = message
        .map(str::to_string)
        .unwrap_or_else(|| format!("delete memory {slug}"));
    let commit_id = backend
        .write_commit(
            handle,
            CommitSpec::mmcp_commit(
                commit_message,
                // `build_tree` interprets `(path, None)` as a
                // delete, so we don't need a separate delete API.
                vec![(mmcp_core::conventions::legacy_memory_path(slug), None)],
                &author.name,
                &author.email,
            ),
        )
        .await?;
    Ok(commit_id)
}

/// Internal: render-agnostic commit helper shared by
/// `create_memory_file` / `update_memory_file`. Keeps the
/// `CommitSpec` construction in one place so the `mmcp_commit`
/// conventions and author plumbing don't drift between create and
/// update.
async fn write_memory_commit(
    backend: &NativeBackend,
    handle: &RepoHandle,
    slug: &str,
    rendered: &str,
    author: &ResolvedAuthor,
    commit_message: String,
) -> Result<String, ImportError> {
    let commit_id = backend
        .write_commit(
            handle,
            CommitSpec::mmcp_commit(
                commit_message,
                vec![(
                    mmcp_core::conventions::legacy_memory_path(slug),
                    Some(rendered.as_bytes().to_vec()),
                )],
                &author.name,
                &author.email,
            ),
        )
        .await?;
    Ok(commit_id)
}

/// Import a memory into a group repo, respecting a caller-chosen
/// create-or-replace policy.
///
/// `override_existing` controls the collision branch:
/// - `false` (strict create): error with
///   [`ImportError::MemoryAlreadyExists`] when the slug is on disk.
///   Used by the tightened MCP `write_memory` path and the CLI
///   `mmcp import` default.
/// - `true` (replace): silently update in place when the slug
///   exists. Used by the MCP `write_memory` tool when the caller
///   passes `override: true`, and by `mmcp import --override`.
///
/// `content` is the full markdown text. If it starts with `+++`
/// fences the frontmatter is parsed from it. Otherwise
/// `synth_frontmatter` must supply name/description/kind.
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

    let memory_file = if content.trim_start().starts_with("+++") {
        MemoryFile::parse(content)?
    } else if let Some(synth) = synth_frontmatter {
        MemoryFile {
            frontmatter: MemoryFrontmatter {
                id: None,
                name: synth.name,
                description: synth.description,
                kind: synth.kind,
                mandatory: false,
                version: None,
                tags: Vec::new(),
                bump_intent: None,
                feature: None,
            },
            body: content.to_string(),
            format: mmcp_core::memory::FrontmatterFormat::TomlPlus,
        }
    } else {
        return Err(ImportError::MissingFrontmatter);
    };

    let rendered = memory_file
        .to_string()
        .map_err(|e| ImportError::Render(e.to_string()))?;

    let message = format!("import memory {slug}");
    let commit_id = if memory_exists(backend, handle, slug).await? {
        if !override_existing {
            return Err(ImportError::MemoryAlreadyExists {
                slug: slug.to_string(),
            });
        }
        update_memory_file(backend, handle, slug, &rendered, author, Some(&message)).await?
    } else {
        create_memory_file(backend, handle, slug, &rendered, author, Some(&message)).await?
    };

    Ok(ImportResult {
        slug: slug.to_string(),
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
/// Strips the memory extension (`.md`) before delegating to the
/// `slug` crate, which handles Unicode normalization (NFD + diacritic
/// stripping) and hyphen collapsing for us.
pub fn slugify_filename(filename: &str) -> String {
    let stem = filename
        .strip_suffix(mmcp_core::conventions::MEMORY_EXTENSION)
        .unwrap_or(filename);
    slug::slugify(stem)
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
    async fn create_memory_file_writes_when_absent() {
        let (backend, handle, _tmp) = test_backend().await;
        let author = test_author();
        let commit_id =
            create_memory_file(&backend, &handle, "new-mem", SAMPLE_RENDERED, &author, None)
                .await
                .expect("create");
        assert!(!commit_id.is_empty());
        assert!(memory_exists(&backend, &handle, "new-mem").await.unwrap());
    }

    #[tokio::test]
    async fn create_memory_file_errors_when_slug_already_exists() {
        let (backend, handle, _tmp) = test_backend().await;
        let author = test_author();
        create_memory_file(&backend, &handle, "taken", SAMPLE_RENDERED, &author, None)
            .await
            .expect("seed");
        let err = create_memory_file(&backend, &handle, "taken", SAMPLE_RENDERED, &author, None)
            .await
            .expect_err("second create must refuse");
        assert!(matches!(err, ImportError::MemoryAlreadyExists { slug } if slug == "taken"));
    }

    #[tokio::test]
    async fn update_memory_file_errors_when_slug_does_not_exist() {
        let (backend, handle, _tmp) = test_backend().await;
        let author = test_author();
        let err = update_memory_file(&backend, &handle, "missing", SAMPLE_RENDERED, &author, None)
            .await
            .expect_err("update on absent slug");
        assert!(matches!(err, ImportError::MemoryNotFound { slug: Some(s), .. } if s == "missing"));
    }

    #[tokio::test]
    async fn update_memory_file_replaces_content_when_slug_exists() {
        let (backend, handle, _tmp) = test_backend().await;
        let author = test_author();
        create_memory_file(
            &backend,
            &handle,
            "editable",
            SAMPLE_RENDERED,
            &author,
            None,
        )
        .await
        .expect("seed");
        let updated = "+++\nname = \"updated\"\ndescription = \"s\"\nkind = \"rule\"\nmandatory = false\ntags = []\n+++\nNew body.\n";
        update_memory_file(&backend, &handle, "editable", updated, &author, None)
            .await
            .expect("update");
        let bytes = backend
            .read_file(&handle, "memories/editable.md", &Rev::head())
            .await
            .expect("read back");
        let text = std::str::from_utf8(&bytes).expect("utf8");
        assert!(text.contains("name = \"updated\""));
        assert!(text.contains("New body."));
    }

    #[tokio::test]
    async fn delete_memory_file_removes_blob_and_advances_main() {
        let (backend, handle, _tmp) = test_backend().await;
        let author = test_author();
        create_memory_file(&backend, &handle, "doomed", SAMPLE_RENDERED, &author, None)
            .await
            .expect("seed");
        assert!(memory_exists(&backend, &handle, "doomed").await.unwrap());
        let commit_id = delete_memory_file(&backend, &handle, "doomed", &author, None)
            .await
            .expect("delete");
        assert!(!commit_id.is_empty());
        assert!(!memory_exists(&backend, &handle, "doomed").await.unwrap());
    }

    #[tokio::test]
    async fn delete_memory_file_errors_when_slug_absent() {
        let (backend, handle, _tmp) = test_backend().await;
        let author = test_author();
        let err = delete_memory_file(&backend, &handle, "ghost", &author, None)
            .await
            .expect_err("delete on absent slug");
        assert!(matches!(err, ImportError::MemoryNotFound { slug: Some(s), .. } if s == "ghost"));
    }

    #[tokio::test]
    async fn import_memory_rejects_collision_without_override() {
        // Tightened CLI / MCP default: `override_existing = false`
        // must surface the collision instead of silently replacing.
        let (backend, handle, _tmp) = test_backend().await;
        let author = test_author();
        import_memory(
            &backend,
            &handle,
            "taken",
            SAMPLE_RENDERED,
            None,
            &author,
            false,
        )
        .await
        .expect("seed");
        let err = import_memory(
            &backend,
            &handle,
            "taken",
            SAMPLE_RENDERED,
            None,
            &author,
            false,
        )
        .await
        .expect_err("second create must refuse");
        assert!(matches!(err, ImportError::MemoryAlreadyExists { slug } if slug == "taken"));
    }

    #[tokio::test]
    async fn import_memory_replaces_when_override_is_true() {
        // `override_existing = true` preserves the old upsert
        // behavior so `mmcp import --override` + MCP `write_memory`
        // with `override: true` keep working.
        let (backend, handle, _tmp) = test_backend().await;
        let author = test_author();
        let first = import_memory(
            &backend,
            &handle,
            "upsert",
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
            "upsert",
            SAMPLE_RENDERED,
            None,
            &author,
            true,
        )
        .await
        .expect("second with override");
        assert_ne!(first.commit_id, second.commit_id);
        assert_eq!(first.slug, second.slug);
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
        assert_eq!(resolved.id, Some(id));
        assert_eq!(resolved.path, format!("memories/rt/{id}.md"));
    }

    #[tokio::test]
    async fn resolve_memory_by_slug_falls_back_to_legacy_path() {
        let (backend, handle, _tmp) = test_backend().await;
        let author = test_author();
        import_memory(
            &backend,
            &handle,
            "legacy",
            SAMPLE_RENDERED,
            None,
            &author,
            false,
        )
        .await
        .expect("seed legacy");

        let resolved = resolve_memory(&backend, &handle, Some("legacy"), None)
            .await
            .expect("resolve");
        assert_eq!(resolved.slug, "legacy");
        assert_eq!(resolved.id, None);
        assert_eq!(resolved.path, "memories/legacy.md");
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
        assert_eq!(resolved.id, Some(id));
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
    async fn import_then_read_round_trips() {
        let (backend, handle, _tmp) = test_backend().await;
        let author = test_author();
        let content = "+++\nname = \"rt\"\ndescription = \"round trip\"\nkind = \"rule\"\nmandatory = true\ntags = [\"test\"]\n+++\n\nRound trip body.\n";
        import_memory(&backend, &handle, "rt-test", content, None, &author, false)
            .await
            .expect("import");

        let bytes = backend
            .read_file(
                &handle,
                "memories/rt-test.md",
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
    }
}
