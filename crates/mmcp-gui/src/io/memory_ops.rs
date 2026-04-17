//! Thin wrappers over `mmcp-store` / `mmcp-git` memory primitives.
//!
//! Centralising these helpers in `io/` keeps the background worker
//! free of git-path fiddling (memory suffix stripping, the memories
//! directory constant, `Rev::head` boilerplate) and gives future
//! consumers one place to extend the surface.
//!
//! Post-FR-028: every primitive routes through `resolve_memory` or
//! `write_memory_by_id` so the GUI works identically against the
//! two-level `memories/<slug>/<uuid>.md` layout and the legacy flat
//! `memories/<slug>.md` fallback that pre-FR-028 groups still hold.

use mmcp_core::memory::MemoryFile;
use mmcp_git::{GitBackend, NativeBackend, RepoHandle, Rev};
use mmcp_store::{
    ResolvedAuthor, delete_file_at_path, resolve_memory, write_file_at_path, write_memory_by_id,
};
use uuid::Uuid;

use crate::error::GuiError;

const MEMORY_EXTENSION: &str = ".md";

/// List every memory slug present on disk for the given group at
/// current `HEAD`. Slugs come from two sources to cover both
/// layouts simultaneously: the two-level `memories/<slug>/` subdirs
/// (FR-028) and any remaining flat `memories/<slug>.md` files.
/// Returns each slug once, sorted, regardless of how many memories
/// share it.
pub async fn list_memory_slugs(
    backend: &NativeBackend,
    handle: &RepoHandle,
) -> Result<Vec<String>, GuiError> {
    let mut slugs: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    // Two-level: every subdir under `memories/` is a slug.
    for subtree in backend
        .list_subtrees(handle, mmcp_core::conventions::MEMORIES_DIR, &Rev::head())
        .await?
    {
        slugs.insert(subtree);
    }
    // Legacy flat: any `.md` directly under `memories/` is a slug.
    for flat in backend
        .list_tree(handle, mmcp_core::conventions::MEMORIES_DIR, &Rev::head())
        .await?
    {
        if let Some(stem) = flat.strip_suffix(MEMORY_EXTENSION) {
            slugs.insert(stem.to_string());
        }
    }
    Ok(slugs.into_iter().collect())
}

/// Read and parse the memory resolved by `slug`. Routes through the
/// shared `resolve_memory` primitive so both the two-level layout
/// and the legacy flat layout are reachable; ambiguous slugs (≥2
/// UUIDs sharing a slug) surface as a store-layer
/// `MemoryAmbiguous` that the GUI can show verbatim.
pub async fn read_memory_body(
    backend: &NativeBackend,
    handle: &RepoHandle,
    slug: &str,
) -> Result<MemoryFile, GuiError> {
    let resolved = resolve_memory(backend, handle, Some(slug), None).await?;
    let bytes = backend
        .read_file(handle, &resolved.path, &Rev::head())
        .await?;
    let text = std::str::from_utf8(&bytes)?;
    Ok(MemoryFile::parse(text)?)
}

/// Create a fresh memory under `slug`. Mints a UUIDv7 and commits
/// at `memories/<slug>/<uuid>.md` via `write_memory_by_id` so the
/// write lands at the canonical FR-028 path. If the incoming
/// `MemoryFile` already carries an id in frontmatter (e.g. from a
/// round-trip through `read_memory_body`), that id is honored;
/// otherwise a fresh one is minted and stamped into the render.
pub async fn create_memory(
    backend: &NativeBackend,
    handle: &RepoHandle,
    slug: &str,
    file: &MemoryFile,
    author: &ResolvedAuthor,
) -> Result<String, GuiError> {
    let id = file.frontmatter.id.unwrap_or_else(Uuid::now_v7);
    let mut file = file.clone();
    file.frontmatter = file.frontmatter.clone().with_id(id);
    let rendered = file
        .to_string()
        .map_err(|e| GuiError::Other(format!("render: {e}")))?;
    Ok(write_memory_by_id(backend, handle, slug, id, &rendered, author, false, None).await?)
}

/// Update an existing memory at its resolved path. Returns
/// `MemoryNotFound` when the slug has no file so callers that
/// forgot a create don't silently produce a new memory here.
pub async fn update_memory(
    backend: &NativeBackend,
    handle: &RepoHandle,
    slug: &str,
    file: &MemoryFile,
    author: &ResolvedAuthor,
) -> Result<String, GuiError> {
    let resolved = resolve_memory(backend, handle, Some(slug), None).await?;
    let mut file = file.clone();
    if let Some(id) = resolved.id {
        file.frontmatter = file.frontmatter.clone().with_id(id);
    }
    let rendered = file
        .to_string()
        .map_err(|e| GuiError::Other(format!("render: {e}")))?;
    Ok(write_file_at_path(backend, handle, &resolved.path, &rendered, author, None).await?)
}

/// Commit a deletion of the resolved memory path.
pub async fn delete_memory(
    backend: &NativeBackend,
    handle: &RepoHandle,
    slug: &str,
    author: &ResolvedAuthor,
) -> Result<String, GuiError> {
    let resolved = resolve_memory(backend, handle, Some(slug), None).await?;
    Ok(delete_file_at_path(backend, handle, &resolved.path, author, None).await?)
}
