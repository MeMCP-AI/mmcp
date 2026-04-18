//! Thin wrappers over `mmcp-store` / `mmcp-git` memory primitives.
//!
//! Centralising these helpers in `io/` keeps the background worker
//! free of git-path fiddling (memory suffix stripping, the memories
//! directory constant, `Rev::head` boilerplate) and gives future
//! consumers one place to extend the surface. Every primitive
//! routes through `resolve_memory` or `write_memory_by_id` so the
//! GUI works against the `memories/<slug>/<uuid>.md` layout.

use mmcp_core::memory::MemoryFile;
use mmcp_git::{GitBackend, NativeBackend, RepoHandle, Rev};
use mmcp_store::{
    ResolvedAuthor, delete_file_at_path, resolve_memory, write_file_at_path, write_memory_by_id,
};
use uuid::Uuid;

use crate::error::GuiError;

/// List every memory slug present on disk for the given group at
/// current `HEAD`. Each subdirectory directly under `memories/` is
/// a slug. A slug with multiple UUID-named files surfaces once —
/// duplicates exist at the file level, not the slug level.
pub async fn list_memory_slugs(
    backend: &NativeBackend,
    handle: &RepoHandle,
) -> Result<Vec<String>, GuiError> {
    let mut slugs = backend
        .list_subtrees(handle, mmcp_core::conventions::MEMORIES_DIR, &Rev::head())
        .await?;
    slugs.sort();
    Ok(slugs)
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
    file.frontmatter = file.frontmatter.clone().with_id(resolved.id);
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
