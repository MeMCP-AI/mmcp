//! Shared tracker-counter helper.
//!
//! Computes the next monotonic ticket number across every tracker
//! memory (features + issues + future tracker kinds) in a group.
//! `add_feature` and `add_issue` both call this so the counter is
//! one space per group — GitHub-style — and a feature and an issue
//! never share a ticket number.
//!
//! Per the global-coding-rules section 13 the helper lives in its
//! own concern-named module so neither tracker surface owns it.
//! Gaps from deletes stay gaps because the helper picks `max + 1`
//! and never reuses the slot.

use mmcp_core::memory::MemoryFile;
use mmcp_git::{GitBackend, NativeBackend, Rev};

use crate::groups::GroupEntry;
use crate::memory::{ImportError, list_memory_slug_dirs};

/// Compute the next ticket number for the group.
///
/// Reads the frontmatter of every memory under `memories/`, looks
/// at `feature.number` and `issue.number`, and returns one more
/// than the maximum observed value. Returns `1` for an empty
/// group. FR-41-aware: nested slug paths are walked recursively
/// via [`list_memory_slug_dirs`].
///
/// Errors only on a hard list / read failure on the underlying
/// git tree; per-memory parse errors are ignored so a single
/// malformed file does not stall the counter.
pub async fn next_ticket_number(
    backend: &NativeBackend,
    entry: &GroupEntry,
) -> Result<u32, ImportError> {
    let rev = Rev::head();
    let slug_dirs = list_memory_slug_dirs(backend, &entry.handle, &rev)
        .await
        .map_err(ImportError::Git)?;
    let mut max = 0u32;
    for slug_dir in slug_dirs {
        for filename in &slug_dir.filenames {
            let path = format!("{}/{filename}", slug_dir.dir);
            let Ok(bytes) = backend.read_file(&entry.handle, &path, &rev).await else {
                continue;
            };
            let Ok(text) = std::str::from_utf8(&bytes) else {
                continue;
            };
            let Ok(mf) = MemoryFile::parse(text) else {
                continue;
            };
            if let Some(meta) = mf.frontmatter.feature.as_ref()
                && let Some(n) = meta.number
            {
                max = max.max(n);
            }
            if let Some(meta) = mf.frontmatter.issue.as_ref()
                && let Some(n) = meta.number
            {
                max = max.max(n);
            }
        }
    }
    Ok(max + 1)
}
