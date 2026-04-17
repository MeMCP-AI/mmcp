//! Thin wrappers over `mmcp-store` / `mmcp-git` memory primitives.
//!
//! Centralising these helpers in `io/` keeps the background worker
//! free of git-path fiddling (memory suffix stripping, the memories
//! directory constant, `Rev::head` boilerplate) and gives future
//! consumers one place to extend the surface.

use mmcp_core::memory::MemoryFile;
use mmcp_git::{GitBackend, NativeBackend, RepoHandle, Rev};
use mmcp_store::{ResolvedAuthor, create_memory_file, delete_memory_file, update_memory_file};

use crate::error::GuiError;

const MEMORY_EXTENSION: &str = ".md";

/// List every memory slug present on disk for the given group at
/// current `HEAD`. Slugs are returned sorted, with the `.md` suffix
/// stripped so callers can render them verbatim.
pub async fn list_memory_slugs(
    backend: &NativeBackend,
    handle: &RepoHandle,
) -> Result<Vec<String>, GuiError> {
    let files = backend
        .list_tree(handle, mmcp_core::conventions::MEMORIES_DIR, &Rev::head())
        .await?;
    let mut slugs: Vec<String> = files
        .into_iter()
        .filter_map(|f| f.strip_suffix(MEMORY_EXTENSION).map(str::to_string))
        .collect();
    slugs.sort();
    Ok(slugs)
}

/// Read and parse the memory at `memories/<slug>.md` for the given
/// group.
pub async fn read_memory_body(
    backend: &NativeBackend,
    handle: &RepoHandle,
    slug: &str,
) -> Result<MemoryFile, GuiError> {
    let path = mmcp_core::conventions::memory_path(slug);
    let bytes = backend.read_file(handle, &path, &Rev::head()).await?;
    let text = std::str::from_utf8(&bytes)?;
    Ok(MemoryFile::parse(text)?)
}

/// Render `file` and call `mmcp_store::create_memory_file` so a
/// strict-create semantics applies — existing slugs produce a typed
/// `MemoryAlreadyExists` error rather than a silent overwrite.
pub async fn create_memory(
    backend: &NativeBackend,
    handle: &RepoHandle,
    slug: &str,
    file: &MemoryFile,
    author: &ResolvedAuthor,
) -> Result<String, GuiError> {
    let rendered = file
        .to_string()
        .map_err(|e| GuiError::Other(format!("render: {e}")))?;
    Ok(create_memory_file(backend, handle, slug, &rendered, author, None).await?)
}

/// Update an existing memory; returns `MemoryNotFound` via
/// [`ImportError`] when the slug is absent rather than creating
/// silently.
pub async fn update_memory(
    backend: &NativeBackend,
    handle: &RepoHandle,
    slug: &str,
    file: &MemoryFile,
    author: &ResolvedAuthor,
) -> Result<String, GuiError> {
    let rendered = file
        .to_string()
        .map_err(|e| GuiError::Other(format!("render: {e}")))?;
    Ok(update_memory_file(backend, handle, slug, &rendered, author, None).await?)
}

/// Commit a deletion.
pub async fn delete_memory(
    backend: &NativeBackend,
    handle: &RepoHandle,
    slug: &str,
    author: &ResolvedAuthor,
) -> Result<String, GuiError> {
    Ok(delete_memory_file(backend, handle, slug, author, None).await?)
}
