//! Memory import - shared core logic for CLI and MCP tool.
//!
//! Writes a memory file into a group's bare git repository.
//! The same `import_memory` function is called by both the
//! `mmcp import` CLI subcommand and the `import_memory` MCP tool.

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use mmcp_core::id::GroupId;
use mmcp_core::memory::{MemoryFile, MemoryFrontmatter, MemoryKind};
use mmcp_git::{CommitSpec, GitBackend, NativeBackend, RepoHandle};
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

/// Errors specific to the import operation.
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
}

/// Import a single memory into a group repository.
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

    let commit_id = backend
        .write_commit(
            handle,
            CommitSpec::mmcp_commit(
                format!("import memory {slug}"),
                vec![(
                    mmcp_core::conventions::memory_path(slug),
                    Some(rendered.into_bytes()),
                )],
                &author.name,
                &author.email,
            ),
        )
        .await?;

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
pub fn slugify_filename(filename: &str) -> String {
    let stem = filename
        .strip_suffix(mmcp_core::conventions::MEMORY_EXTENSION)
        .unwrap_or(filename);
    let mut slug = String::with_capacity(stem.len());
    for ch in stem.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
        } else if !slug.is_empty() && !slug.ends_with('-') {
            slug.push('-');
        }
    }
    // Trim trailing hyphens.
    while slug.ends_with('-') {
        slug.pop();
    }
    slug
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

// ── CLI entry point ─────────────────────────────────────────────

/// CLI entry point for `mmcp import`.
pub async fn run(
    group: String,
    file: Option<PathBuf>,
    dir: Option<PathBuf>,
    slug_override: Option<String>,
    name: Option<String>,
    description: Option<String>,
    kind: Option<String>,
) -> Result<()> {
    let mmcp_home = crate::home::MmcpHome::discover()?;
    let (backend, group_index) = mmcp_home.init_backend().await?;
    let author = mmcp_home.resolve_author();

    let entry = resolve_group(&group_index, &group)
        .await
        .with_context(|| format!("resolving group '{group}'"))?;

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
        let result = import_memory(&backend, &entry.handle, &slug, &content, synth, &author).await?;
        println!("imported {} (commit {})", result.slug, result.commit_id);
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
            match import_memory(&backend, &entry.handle, &slug, &content, synth.clone(), &author).await {
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
        let result = import_memory(&backend, &handle, "test-mem", content, None, &author)
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
        let result = import_memory(&backend, &handle, "plain-mem", content, synth, &author)
            .await
            .expect("import");
        assert_eq!(result.slug, "plain-mem");
    }

    #[tokio::test]
    async fn import_without_frontmatter_or_synth_fails() {
        let (backend, handle, _tmp) = test_backend().await;
        let author = test_author();
        let content = "No frontmatter here.\n";
        let err = import_memory(&backend, &handle, "bad", content, None, &author)
            .await
            .unwrap_err();
        assert!(matches!(err, ImportError::MissingFrontmatter));
    }

    #[tokio::test]
    async fn import_then_read_round_trips() {
        let (backend, handle, _tmp) = test_backend().await;
        let author = test_author();
        let content = "+++\nname = \"rt\"\ndescription = \"round trip\"\nkind = \"rule\"\nmandatory = true\ntags = [\"test\"]\n+++\n\nRound trip body.\n";
        import_memory(&backend, &handle, "rt-test", content, None, &author)
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
