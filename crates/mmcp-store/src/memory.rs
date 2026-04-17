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
    /// which signals a missing group altogether.
    #[error("memory '{slug}' does not exist in this group")]
    MemoryNotFound { slug: String },
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
            slug: slug.to_string(),
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
            slug: slug.to_string(),
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
        assert!(matches!(err, ImportError::MemoryNotFound { slug } if slug == "missing"));
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
        assert!(matches!(err, ImportError::MemoryNotFound { slug } if slug == "ghost"));
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
