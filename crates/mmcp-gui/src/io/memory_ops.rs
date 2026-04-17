//! Thin wrappers over `mmcp-store` / `mmcp-git` read primitives.
//!
//! Centralising these helpers in `io/` keeps the background worker
//! free of git-path fiddling (memory suffix stripping, the memories
//! directory constant, `Rev::head` boilerplate) and gives future
//! consumers — e.g. the write path in phase 5 — one place to extend
//! the read surface when new operations are added.

use mmcp_core::memory::MemoryFile;
use mmcp_git::{GitBackend, NativeBackend, RepoHandle, Rev};

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
/// group. Surfaces frontmatter parse errors verbatim so the viewer
/// can show a diagnostic rather than silently falling back.
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
