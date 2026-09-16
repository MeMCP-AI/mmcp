//! Read-only back-reference report, scanned after a cross-group move.
//!
//! A cross-group move ([`crate::memory_move::move_memory_across_groups`])
//! never rewrites another memory's body: a documentation-style
//! `[[...]]` link is free text, not a typed cross-reference the
//! move machinery could find and update. This module scans every
//! locally mirrored group's memory bodies for a link a caller may
//! want to correct by hand, and reports it, named by group slug,
//! without touching a single byte.

use uuid::Uuid;

use mmcp_core::memory::{BodyLink, find_body_links};
use mmcp_git::{NativeBackend, Rev};

use crate::groups::GroupIndex;
use crate::memory::{ImportError, list_all_memory_files, parse_memory_file_bytes};

/// Note attached to every [`BackReferenceReport`]: server-side push
/// acceptance of a memory moved across groups is not yet implemented.
pub const SYNC_PUSH_REFUSED_NOTE: &str =
    "a sync push of this memory is refused by the server until issue #457 lands";

/// Which link form a [`BackReference`] found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackReferenceKind {
    /// `[[<source-group-uuid>:<pre-move-slug>]]`, found in any
    /// locally mirrored group's memory body.
    CrossGroupLink,
    /// Bare `[[<pre-move-slug>]]`, found inside the source group,
    /// now dangling: that slug no longer resolves there.
    DanglingSameGroupLink,
}

/// One `[[...]]` link a reader may want to correct after a move.
#[derive(Debug, Clone)]
pub struct BackReference {
    pub group_slug: String,
    pub memory_slug: String,
    pub memory_id: Uuid,
    pub kind: BackReferenceKind,
}

/// Outcome of [`scan_back_references`].
#[derive(Debug, Clone)]
pub struct BackReferenceReport {
    pub references: Vec<BackReference>,
    pub sync_push_note: &'static str,
}

/// Scan every locally mirrored group for a `[[...]]` link to the
/// memory that just moved out of the group at `source_group`.
///
/// `pre_move_slug` is the slug the memory had in the source group
/// before the move: a documentation-style link always names that
/// old address, since this loose convention is never rewritten by
/// any mmcp operation (unlike the typed `refs` cross-reference list,
/// which keeps resolving by id).
pub async fn scan_back_references(
    backend: &NativeBackend,
    groups: &GroupIndex,
    source_group: Uuid,
    pre_move_slug: &str,
) -> Result<BackReferenceReport, ImportError> {
    let mut references = Vec::new();
    for entry in groups.list().await {
        let rev = Rev::head();
        let files = list_all_memory_files(backend, &entry.handle, &rev)
            .await
            .map_err(ImportError::Git)?;
        let paths: Vec<String> = files.iter().map(|f| f.path.clone()).collect();
        let batch = backend
            .read_files(&entry.handle, paths, &rev)
            .await
            .map_err(ImportError::Git)?;
        let bytes_by_path: std::collections::HashMap<_, _> = batch.into_iter().collect();
        let is_source_group = entry.handle.group_id == source_group;

        for file_ref in &files {
            let Some(Ok(bytes)) = bytes_by_path.get(&file_ref.path) else {
                continue;
            };
            let Ok(parsed) = parse_memory_file_bytes(bytes, &file_ref.path) else {
                continue;
            };
            for link in find_body_links(&parsed.body) {
                let kind = match link {
                    BodyLink::CrossGroup { group, slug }
                        if group == source_group && slug == pre_move_slug =>
                    {
                        BackReferenceKind::CrossGroupLink
                    }
                    BodyLink::SameGroup { slug } if is_source_group && slug == pre_move_slug => {
                        BackReferenceKind::DanglingSameGroupLink
                    }
                    _ => continue,
                };
                references.push(BackReference {
                    group_slug: entry.manifest.slug.clone(),
                    memory_slug: file_ref.slug.clone(),
                    memory_id: file_ref.id,
                    kind,
                });
            }
        }
    }
    Ok(BackReferenceReport {
        references,
        sync_push_note: SYNC_PUSH_REFUSED_NOTE,
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use crate::memory::import_memory;
    use crate::memory_move::{CrossGroupMoveOptions, move_memory_across_groups};
    use crate::testing::ScratchHome;
    use mmcp_core::memory::{FrontmatterFormat, MemoryFile, MemoryFrontmatter, MemoryKind};

    #[tokio::test]
    async fn scan_finds_cross_group_and_dangling_same_group_links() {
        let scratch = ScratchHome::new().await.expect("scratch home");
        let source = scratch
            .seed_group("back-ref-source")
            .await
            .expect("seed source");
        let target = scratch
            .seed_group("back-ref-target")
            .await
            .expect("seed target");
        let other = scratch
            .seed_group("back-ref-other")
            .await
            .expect("seed other");
        let source_entry = scratch
            .groups()
            .get(&source.group_id)
            .await
            .expect("source entry");
        let target_entry = scratch
            .groups()
            .get(&target.group_id)
            .await
            .expect("target entry");
        let other_entry = scratch
            .groups()
            .get(&other.group_id)
            .await
            .expect("other entry");

        let moving = MemoryFile {
            frontmatter: MemoryFrontmatter::new("moving", "will move", MemoryKind::Scratch),
            body: "moving body\n".to_string(),
            format: FrontmatterFormat::TomlPlus,
        };
        let seeded = import_memory(
            scratch.backend(),
            &source_entry.handle,
            "moving",
            &moving.to_string().expect("render"),
            None,
            scratch.author(),
            false,
        )
        .await
        .expect("seed moving memory");

        // A same-group sibling in the source that references the
        // moving memory by its bare slug: dangles after the move.
        let sibling = MemoryFile {
            frontmatter: MemoryFrontmatter::new(
                "sibling",
                "references moving",
                MemoryKind::Scratch,
            ),
            body: "see [[moving]] for context\n".to_string(),
            format: FrontmatterFormat::TomlPlus,
        };
        import_memory(
            scratch.backend(),
            &source_entry.handle,
            "sibling",
            &sibling.to_string().expect("render"),
            None,
            scratch.author(),
            false,
        )
        .await
        .expect("seed sibling memory");

        // An unrelated group whose memory references the moving
        // memory by its cross-group `[[group:slug]]` form.
        let referrer = MemoryFile {
            frontmatter: MemoryFrontmatter::new(
                "referrer",
                "cross-group link",
                MemoryKind::Scratch,
            ),
            body: format!("see [[{}:moving]] for context\n", source.group_id.as_uuid()),
            format: FrontmatterFormat::TomlPlus,
        };
        import_memory(
            scratch.backend(),
            &other_entry.handle,
            "referrer",
            &referrer.to_string().expect("render"),
            None,
            scratch.author(),
            false,
        )
        .await
        .expect("seed referrer memory");

        move_memory_across_groups(
            scratch.backend(),
            &source_entry,
            &target_entry,
            None,
            Some(seeded.id),
            scratch.author(),
            CrossGroupMoveOptions::default(),
        )
        .await
        .expect("move across groups");

        let report = scan_back_references(
            scratch.backend(),
            scratch.groups(),
            *source.group_id.as_uuid(),
            "moving",
        )
        .await
        .expect("scan back references");

        assert!(
            report
                .references
                .iter()
                .any(|r| r.group_slug == "back-ref-other"
                    && r.memory_slug == "referrer"
                    && r.kind == BackReferenceKind::CrossGroupLink),
            "must find the cross-group link: {:?}",
            report.references
        );
        assert!(
            report
                .references
                .iter()
                .any(|r| r.group_slug == "back-ref-source"
                    && r.memory_slug == "sibling"
                    && r.kind == BackReferenceKind::DanglingSameGroupLink),
            "must find the dangling same-group link: {:?}",
            report.references
        );
        assert!(report.sync_push_note.contains("457"));
    }
}
