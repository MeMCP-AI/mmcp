//! Shared memory CRUD primitives for the CLI and the MCP tools.
//!
//! Three typed primitives (`create_memory_file`, `update_memory_file`,
//! `delete_memory_file`) wrap `NativeBackend::write_commit` with
//! existence checks so the MCP tools (`write_memory`, `edit_memory`,
//! `delete_memory`) can enforce their strict CRUD contracts without
//! each tool re-implementing the probe. The `import_memory` upsert
//! wrapper is retained so the CLI `mmcp import` path keeps its
//! operator-driven overwrite-on-collision behavior (tightening that
//! path happens in a separate track alongside the MCP
//! `write_memory` rework).

use std::io::IsTerminal;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use inquire::Confirm;
use mmcp_core::id::GroupId;
use mmcp_core::memory::{MemoryFile, MemoryFrontmatter, MemoryKind};
use mmcp_git::{CommitSpec, GitBackend, GitError, NativeBackend, RepoHandle, Rev};
use uuid::Uuid;

use crate::state::{GroupEntry, GroupIndex};

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
    #[error("invalid slug '{0}': must be 1-128 chars, lowercase alphanumeric with hyphens, no leading/trailing hyphens")]
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
    let path = mmcp_core::conventions::memory_path(slug);
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
    author: &crate::home::ResolvedAuthor,
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
    author: &crate::home::ResolvedAuthor,
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
    author: &crate::home::ResolvedAuthor,
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
                vec![(mmcp_core::conventions::memory_path(slug), None)],
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
    author: &crate::home::ResolvedAuthor,
    commit_message: String,
) -> Result<String, ImportError> {
    let commit_id = backend
        .write_commit(
            handle,
            CommitSpec::mmcp_commit(
                commit_message,
                vec![(
                    mmcp_core::conventions::memory_path(slug),
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
    author: &crate::home::ResolvedAuthor,
    override_existing: bool,
) -> Result<ImportResult, ImportError> {
    validate_slug(slug)?;

    let memory_file = if content.trim_start().starts_with("+++") {
        MemoryFile::parse(content)?
    } else if let Some(synth) = synth_frontmatter {
        MemoryFile {
            frontmatter: MemoryFrontmatter {
                name: synth.name,
                description: synth.description,
                kind: synth.kind,
                mandatory: false,
                version: None,
                tags: Vec::new(),
                bump_intent: None,
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

/// CLI-side parity of the MCP `ensure_not_protected` guard.
///
/// When the target group is protected, prompts the operator with
/// `inquire::Confirm` on a TTY (default: no). Non-TTY invocations
/// must pass `force = true` explicitly so scripted imports never
/// silently poke at protected groups. Unprotected groups are a
/// no-op — the operator's `mmcp import` intent is the confirmation.
pub fn protected_confirm(entry: &GroupEntry, force: bool) -> Result<()> {
    if !entry.manifest.protected {
        return Ok(());
    }
    eprintln!(
        "notice: group `{}` is marked protected; writes into it are audited.",
        entry.manifest.slug,
    );
    if force {
        return Ok(());
    }
    if !std::io::stdin().is_terminal() {
        bail!(
            "group `{}` is protected; pass --force on non-TTY invocations to confirm the write",
            entry.manifest.slug,
        );
    }
    let prompt = format!(
        "Writing into protected group `{}` — continue?",
        entry.manifest.slug,
    );
    let confirmed = Confirm::new(&prompt)
        .with_default(false)
        .prompt()
        .context("reading protected-group confirmation from TTY")?;
    if !confirmed {
        bail!("aborted: operator declined to write into protected group");
    }
    Ok(())
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

// ── CLI entry point ─────────────────────────────────────────────

/// CLI entry point for `mmcp import`.
///
/// `override_existing` mirrors the MCP `write_memory` tool's
/// `override` argument: `false` (default) is strict CREATE and
/// errors with a user-facing hint when the slug is already on
/// disk; `true` replaces the file in place. The collision error
/// names both `--override` and `mmcp__edit_memory` so operators
/// see the two escape hatches explicitly.
#[allow(clippy::too_many_arguments)]
pub async fn run(
    group: String,
    file: Option<PathBuf>,
    dir: Option<PathBuf>,
    slug_override: Option<String>,
    name: Option<String>,
    description: Option<String>,
    kind: Option<String>,
    override_existing: bool,
    force: bool,
) -> Result<()> {
    let mmcp_home = crate::home::MmcpHome::discover()?;
    let (backend, group_index) = crate::home::init_backend(&mmcp_home).await?;
    let author = mmcp_home.resolve_author();

    let entry = resolve_group(&group_index, &group)
        .await
        .with_context(|| format!("resolving group '{group}'"))?;

    protected_confirm(&entry, force)?;

    let synth = match (name, description, kind) {
        (Some(n), Some(d), Some(k)) => Some(SynthFrontmatter {
            name: n,
            description: d,
            kind: parse_kind(&k)?,
        }),
        (None, None, None) => None,
        _ => bail!("--name, --description, and --kind must all be provided together or all omitted"),
    };

    if let Some(path) = file {
        let content = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        let slug = slug_override.unwrap_or_else(|| {
            slugify_filename(
                path.file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("unnamed"),
            )
        });
        match import_memory(
            &backend,
            &entry.handle,
            &slug,
            &content,
            synth,
            &author,
            override_existing,
        )
        .await
        {
            Ok(result) => {
                println!("imported {} (commit {})", result.slug, result.commit_id);
            }
            Err(ImportError::MemoryAlreadyExists { slug }) => {
                bail!(
                    "memory `{slug}` already exists in group `{group}`; pass --override to replace it, or use `mmcp__edit_memory` for partial updates"
                );
            }
            Err(err) => return Err(err.into()),
        }
    } else if let Some(dir_path) = dir {
        let mut count = 0;
        let mut entries: Vec<_> = std::fs::read_dir(&dir_path)
            .with_context(|| format!("reading directory {}", dir_path.display()))?
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.path()
                    .extension()
                    .is_some_and(|ext| ext == "md")
            })
            .collect();
        entries.sort_by_key(|e| e.file_name());

        for entry_file in entries {
            let path = entry_file.path();
            let slug = slugify_filename(
                path.file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("unnamed"),
            );
            let content = std::fs::read_to_string(&path)
                .with_context(|| format!("reading {}", path.display()))?;
            match import_memory(
                &backend,
                &entry.handle,
                &slug,
                &content,
                synth.clone(),
                &author,
                override_existing,
            )
            .await
            {
                Ok(result) => {
                    println!("imported {} (commit {})", result.slug, result.commit_id);
                    count += 1;
                }
                Err(err) => {
                    eprintln!("skipped {}: {err}", path.display());
                }
            }
        }
        println!("{count} memories imported");
    } else {
        bail!("either --file or --dir is required");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use mmcp_core::manifest::GroupManifest;
    use tempfile::TempDir;

    fn test_author() -> crate::home::ResolvedAuthor {
        crate::home::ResolvedAuthor {
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
        assert_eq!(slugify_filename("global_coding_rules.md"), "global-coding-rules");
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
        let content = "+++\nname = \"test\"\ndescription = \"a test\"\nkind = \"rule\"\n+++\n\nBody here.\n";
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
        let result = import_memory(&backend, &handle, "plain-mem", content, synth, &author, false)
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
        let commit_id = create_memory_file(&backend, &handle, "new-mem", SAMPLE_RENDERED, &author, None)
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
        let err =
            update_memory_file(&backend, &handle, "missing", SAMPLE_RENDERED, &author, None)
                .await
                .expect_err("update on absent slug");
        assert!(matches!(err, ImportError::MemoryNotFound { slug } if slug == "missing"));
    }

    #[tokio::test]
    async fn update_memory_file_replaces_content_when_slug_exists() {
        let (backend, handle, _tmp) = test_backend().await;
        let author = test_author();
        create_memory_file(&backend, &handle, "editable", SAMPLE_RENDERED, &author, None)
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
        import_memory(&backend, &handle, "taken", SAMPLE_RENDERED, None, &author, false)
            .await
            .expect("seed");
        let err = import_memory(&backend, &handle, "taken", SAMPLE_RENDERED, None, &author, false)
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
        let first = import_memory(&backend, &handle, "upsert", SAMPLE_RENDERED, None, &author, false)
            .await
            .expect("first");
        let second = import_memory(&backend, &handle, "upsert", SAMPLE_RENDERED, None, &author, true)
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
