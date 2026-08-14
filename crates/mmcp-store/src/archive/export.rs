//! Snapshot export: package one or more groups into a portable tar
//! archive whose layout mirrors the on-disk repo minus git internals.

use std::io::Write;
use std::path::{Path, PathBuf};

use flate2::Compression;
use flate2::write::GzEncoder;
use mmcp_core::manifest::MANIFEST_FILENAME;
use mmcp_core::memory::MemoryFile;
use mmcp_git::{GitBackend, NativeBackend, Rev};

use crate::groups::GroupEntry;
use crate::memory::{ImportError, list_all_memory_files, read_frontmatters_in_group};

use super::error::ArchiveError;
use super::filter::MemoryFilter;
use super::manifest::{
    ARCHIVE_FORMAT_VERSION, ARCHIVE_GIT_DIR, ARCHIVE_GROUPS_DIR, ARCHIVE_MANIFEST_FILENAME,
    ArchiveManifest, ArchiveMode, ArchivedGroupMeta,
};

/// Unix mode bits stamped on every archive entry: owner read/write,
/// group/other read. Archived memories are data, never executable.
const ARCHIVE_ENTRY_MODE: u32 = 0o644;

/// Fixed modification time (epoch seconds) on every entry so two
/// exports of identical store content differ only by the manifest's
/// own `created_at` stamp, not by per-file mtimes.
const ARCHIVE_ENTRY_MTIME: u64 = 0;

/// Top-level bare-repo entries excluded from a history capture:
/// `hooks/` are sample executables and `logs/` are local reflogs;
/// neither belongs in a portable backup.
const GIT_EXCLUDED_TOP: [&str; 2] = ["hooks", "logs"];

/// Knobs for an export run. `Default`-derived so call sites set only
/// the toggles they care about.
#[derive(Debug, Clone, Default)]
pub struct ExportOptions {
    /// gzip the tar stream (pure-Rust flate2). Off means a plain tar.
    pub gzip: bool,
    /// Facet filter narrowing which memories are packed. Empty matches
    /// every memory in the selected groups. Snapshot mode only.
    pub filter: MemoryFilter,
    /// What to capture: a HEAD snapshot (default) or each group's full
    /// git history (the bare repo, verbatim).
    pub mode: ArchiveMode,
}

/// Package `groups` into a snapshot archive written to `writer`.
///
/// Each group contributes its verbatim `.mmcp.toml` plus every memory
/// file at HEAD; feature and issue memories ride along as ordinary
/// memory files. Returns the table of contents that was written so
/// callers can report the per-group counts.
pub async fn export_archive<W: Write>(
    backend: &NativeBackend,
    groups: &[GroupEntry],
    options: &ExportOptions,
    writer: W,
) -> Result<ArchiveManifest, ArchiveError> {
    // Read every group's manifest and memory files at HEAD into memory
    // first, then write the tar synchronously, so the async git reads
    // and the sync tar encoding stay cleanly separated.
    let mut group_metas = Vec::with_capacity(groups.len());
    let mut entries: Vec<(String, Vec<u8>)> = Vec::new();

    for group in groups {
        let group_id = group.handle.group_id;
        let base = format!("{ARCHIVE_GROUPS_DIR}/{group_id}");

        // The verbatim manifest rides in both modes so a reader can
        // list a group and its scope without unpacking the payload.
        let manifest_bytes = backend
            .read_file(&group.handle, MANIFEST_FILENAME, &Rev::Head)
            .await?;
        entries.push((
            format!("{base}/{MANIFEST_FILENAME}"),
            manifest_bytes.to_vec(),
        ));

        let memory_count = match options.mode {
            ArchiveMode::Snapshot => {
                pack_snapshot_memories(backend, group, &base, &options.filter, &mut entries).await?
            }
            ArchiveMode::History => {
                let git_base = format!("{base}/{ARCHIVE_GIT_DIR}");
                for (rel, bytes) in pack_git_dir(&backend.repo_path(group_id)).await? {
                    entries.push((format!("{git_base}/{rel}"), bytes));
                }
                // Informational only: the HEAD memory count for listing.
                let files = list_all_memory_files(backend, &group.handle, &Rev::Head).await?;
                u32::try_from(files.len()).unwrap_or(u32::MAX)
            }
        };

        group_metas.push(ArchivedGroupMeta {
            group_id,
            slug: group.manifest.slug.clone(),
            display_name: group.manifest.display_name.clone(),
            memory_count,
        });
    }

    let manifest = ArchiveManifest {
        format_version: ARCHIVE_FORMAT_VERSION,
        mode: options.mode,
        mmcp_version: env!("CARGO_PKG_VERSION").to_string(),
        created_at: jiff::Timestamp::now().to_string(),
        groups: group_metas,
    };

    write_archive(&manifest, &entries, writer, options.gzip)?;
    Ok(manifest)
}

/// Pack a group's HEAD memory files (filtered) under `base`, returning
/// the count packed. The snapshot half of [`export_archive`].
async fn pack_snapshot_memories(
    backend: &NativeBackend,
    group: &GroupEntry,
    base: &str,
    filter: &MemoryFilter,
    entries: &mut Vec<(String, Vec<u8>)>,
) -> Result<u32, ArchiveError> {
    let files = list_all_memory_files(backend, &group.handle, &Rev::Head).await?;
    let mut packed: u32 = 0;
    for file in &files {
        let bytes = backend
            .read_file(&group.handle, &file.path, &Rev::Head)
            .await?;
        if !filter.is_empty() {
            let text = std::str::from_utf8(&bytes).map_err(|source| ArchiveError::NotUtf8 {
                path: file.path.clone(),
                source,
            })?;
            let parsed = MemoryFile::parse(text).map_err(ImportError::Parse)?;
            if !filter.matches(&file.slug, &parsed.frontmatter, &parsed.body) {
                continue;
            }
        }
        entries.push((format!("{base}/{}", file.path), bytes.to_vec()));
        packed = packed.saturating_add(1);
    }
    Ok(packed)
}

/// Walk a group's bare repository and return every git file as
/// `(forward-slash relative path, bytes)`, sorted for determinism.
/// `hooks/` and `logs/` are skipped. Offloaded to a blocking task
/// since it reads the disk synchronously.
async fn pack_git_dir(repo_path: &Path) -> Result<Vec<(String, Vec<u8>)>, ArchiveError> {
    let repo_path = repo_path.to_path_buf();
    tokio::task::spawn_blocking(move || -> Result<Vec<(String, Vec<u8>)>, ArchiveError> {
        let mut out = Vec::new();
        walk_git_dir(&repo_path, &repo_path, &mut out)?;
        out.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(out)
    })
    .await
    .map_err(|e| ArchiveError::Malformed {
        detail: format!("git walk task failed: {e}"),
    })?
}

/// Recursive helper for [`pack_git_dir`]; `root` anchors relative paths.
fn walk_git_dir(
    root: &Path,
    dir: &Path,
    out: &mut Vec<(String, Vec<u8>)>,
) -> Result<(), ArchiveError> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let rel = path.strip_prefix(root).unwrap_or(&path);
        let excluded = rel
            .components()
            .next()
            .and_then(|c| c.as_os_str().to_str())
            .is_some_and(|top| GIT_EXCLUDED_TOP.contains(&top));
        if excluded {
            continue;
        }
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            walk_git_dir(root, &path, out)?;
        } else if file_type.is_file() {
            let rel_str = rel.to_string_lossy().replace('\\', "/");
            out.push((rel_str, std::fs::read(&path)?));
        }
    }
    Ok(())
}

/// Write the table of contents plus every entry to a (optionally
/// gzip-wrapped) tar stream.
fn write_archive<W: Write>(
    manifest: &ArchiveManifest,
    entries: &[(String, Vec<u8>)],
    writer: W,
    gzip: bool,
) -> Result<(), ArchiveError> {
    if gzip {
        let encoder = GzEncoder::new(writer, Compression::default());
        let mut builder = tar::Builder::new(encoder);
        populate(&mut builder, manifest, entries)?;
        // `into_inner` writes the tar trailer and hands back the gzip
        // encoder; `finish` flushes the gzip stream and footer.
        builder.into_inner()?.finish()?;
    } else {
        let mut builder = tar::Builder::new(writer);
        populate(&mut builder, manifest, entries)?;
        builder.into_inner()?.flush()?;
    }
    Ok(())
}

/// Append the `archive.toml` table of contents first (so a streaming
/// reader hits it early) followed by every group/memory entry.
fn populate<W: Write>(
    builder: &mut tar::Builder<W>,
    manifest: &ArchiveManifest,
    entries: &[(String, Vec<u8>)],
) -> Result<(), ArchiveError> {
    let toc = manifest.to_toml()?;
    append_bytes(builder, ARCHIVE_MANIFEST_FILENAME, toc.as_bytes())?;
    for (path, data) in entries {
        append_bytes(builder, path, data)?;
    }
    Ok(())
}

/// Append one regular-file entry with a deterministic header.
fn append_bytes<W: Write>(
    builder: &mut tar::Builder<W>,
    path: &str,
    data: &[u8],
) -> Result<(), ArchiveError> {
    let mut header = tar::Header::new_gnu();
    header.set_entry_type(tar::EntryType::Regular);
    header.set_size(data.len() as u64);
    header.set_mode(ARCHIVE_ENTRY_MODE);
    header.set_mtime(ARCHIVE_ENTRY_MTIME);
    // `append_data` sets the path (long-name aware) and the checksum.
    builder.append_data(&mut header, path, data)?;
    Ok(())
}

/// Export `groups` to `path` atomically: pack into a sibling temp file
/// and rename onto `path` only on success, so a mid-export failure
/// never truncates or leaves a partial file at the operator's chosen
/// destination. The shared entry point for the CLI, MCP, and GUI
/// surfaces so all three publish archives the same way.
pub async fn export_archive_to_path(
    backend: &NativeBackend,
    groups: &[GroupEntry],
    options: &ExportOptions,
    path: &Path,
) -> Result<ArchiveManifest, ArchiveError> {
    let tmp = temp_sibling(path);
    let file = std::fs::File::create(&tmp)?;
    let manifest = match export_archive(backend, groups, options, file).await {
        Ok(manifest) => manifest,
        Err(err) => {
            let _ = std::fs::remove_file(&tmp);
            return Err(err);
        }
    };
    if let Err(err) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(err.into());
    }
    Ok(manifest)
}

/// Distinct tags across the memories of `groups`, sorted. Backs the
/// export dialog's tag autocomplete (the universe of tags a user can
/// filter by). Malformed frontmatter is skipped rather than failing
/// the whole listing.
pub async fn collect_group_tags(
    backend: &NativeBackend,
    groups: &[GroupEntry],
) -> Result<Vec<String>, ArchiveError> {
    let mut tags: Vec<String> = Vec::new();
    for group in groups {
        let entries = read_frontmatters_in_group(backend, &group.handle, &Rev::Head).await?;
        for entry in entries {
            if let Ok(frontmatter) = entry.frontmatter {
                for tag in frontmatter.tags {
                    if !tags.iter().any(|t| t == &tag) {
                        tags.push(tag);
                    }
                }
            }
        }
    }
    tags.sort();
    Ok(tags)
}

/// A same-directory temp path for the atomic export. Same directory so
/// the rename stays on one filesystem; the pid keeps concurrent
/// exports from colliding on the staging file.
fn temp_sibling(path: &Path) -> PathBuf {
    let mut name = path
        .file_name()
        .map(std::ffi::OsString::from)
        .unwrap_or_else(|| std::ffi::OsString::from("archive"));
    name.push(format!(".{}.tmp", std::process::id()));
    match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.join(name),
        _ => PathBuf::from(name),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::{SynthFrontmatter, import_memory};
    use crate::testing::ScratchHome;
    use mmcp_core::memory::MemoryKind;
    use std::io::Read;

    #[tokio::test]
    async fn export_includes_manifest_and_memory_entries() {
        let home = ScratchHome::new().await.expect("scratch home");
        let seeded = home.seed_group("team").await.expect("seed group");
        let entry = home
            .groups()
            .get(&seeded.group_id)
            .await
            .expect("group entry");

        import_memory(
            home.backend(),
            &entry.handle,
            "note",
            "Snapshot body.",
            Some(SynthFrontmatter {
                name: "A note".to_string(),
                description: "desc".to_string(),
                kind: MemoryKind::Reference,
            }),
            home.author(),
            false,
        )
        .await
        .expect("import memory");

        let mut buf = Vec::new();
        let manifest = export_archive(
            home.backend(),
            &[entry],
            &ExportOptions::default(),
            &mut buf,
        )
        .await
        .expect("export");

        assert_eq!(manifest.groups.len(), 1);
        assert_eq!(manifest.groups[0].slug, "team");
        assert_eq!(manifest.groups[0].memory_count, 1);

        // The produced tar must carry the table of contents, the group
        // manifest, and exactly one memory file.
        let mut archive = tar::Archive::new(&buf[..]);
        let mut paths = Vec::new();
        let mut toc = String::new();
        for archive_entry in archive.entries().expect("entries") {
            let mut e = archive_entry.expect("entry");
            let path = e.path().expect("path").to_string_lossy().into_owned();
            if path == ARCHIVE_MANIFEST_FILENAME {
                e.read_to_string(&mut toc).expect("read toc");
            }
            paths.push(path);
        }

        let gid = seeded.group_id.as_uuid();
        assert!(paths.iter().any(|p| p == ARCHIVE_MANIFEST_FILENAME));
        assert!(
            paths
                .iter()
                .any(|p| *p == format!("groups/{gid}/.mmcp.toml"))
        );
        assert!(
            paths
                .iter()
                .any(|p| p.starts_with(&format!("groups/{gid}/memories/note/"))
                    && p.ends_with(".md")),
            "memory entry missing; got {paths:?}",
        );

        let parsed = ArchiveManifest::from_toml(&toc).expect("parse toc");
        assert_eq!(parsed.format_version, ARCHIVE_FORMAT_VERSION);
        assert_eq!(parsed.total_memory_count(), 1);
    }

    #[tokio::test]
    async fn history_export_packs_the_bare_repo() {
        let home = ScratchHome::new().await.expect("scratch home");
        let seeded = home.seed_group("team").await.expect("seed group");
        let entry = home
            .groups()
            .get(&seeded.group_id)
            .await
            .expect("group entry");
        import_memory(
            home.backend(),
            &entry.handle,
            "note",
            "Body.",
            Some(SynthFrontmatter {
                name: "n".to_string(),
                description: "d".to_string(),
                kind: MemoryKind::Reference,
            }),
            home.author(),
            false,
        )
        .await
        .expect("import memory");

        let mut buf = Vec::new();
        let manifest = export_archive(
            home.backend(),
            &[entry],
            &ExportOptions {
                mode: ArchiveMode::History,
                ..Default::default()
            },
            &mut buf,
        )
        .await
        .expect("export");
        assert_eq!(manifest.mode, ArchiveMode::History);

        let gid = seeded.group_id.as_uuid();
        let mut archive = tar::Archive::new(&buf[..]);
        let paths: Vec<String> = archive
            .entries()
            .expect("entries")
            .map(|e| {
                e.expect("entry")
                    .path()
                    .expect("path")
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();

        // The verbatim manifest (for listing) plus the bare repo: HEAD
        // and at least one object must be present.
        assert!(
            paths
                .iter()
                .any(|p| *p == format!("groups/{gid}/.mmcp.toml"))
        );
        assert!(paths.iter().any(|p| *p == format!("groups/{gid}/git/HEAD")));
        assert!(
            paths
                .iter()
                .any(|p| p.starts_with(&format!("groups/{gid}/git/objects/"))),
            "no git objects packed; got {paths:?}",
        );
        assert!(!paths.iter().any(|p| p.contains("/git/hooks/")));
    }

    #[tokio::test]
    async fn export_memory_slug_filter_includes_only_selected() {
        let home = ScratchHome::new().await.expect("scratch home");
        let seeded = home.seed_group("team").await.expect("seed group");
        let entry = home
            .groups()
            .get(&seeded.group_id)
            .await
            .expect("group entry");
        for slug in ["keep", "drop"] {
            import_memory(
                home.backend(),
                &entry.handle,
                slug,
                "Body.",
                Some(SynthFrontmatter {
                    name: slug.to_string(),
                    description: "desc".to_string(),
                    kind: MemoryKind::Reference,
                }),
                home.author(),
                false,
            )
            .await
            .expect("import memory");
        }

        let mut buf = Vec::new();
        let manifest = export_archive(
            home.backend(),
            &[entry],
            &ExportOptions {
                filter: MemoryFilter {
                    slugs: vec!["keep".to_string()],
                    ..Default::default()
                },
                ..Default::default()
            },
            &mut buf,
        )
        .await
        .expect("export");

        assert_eq!(manifest.groups[0].memory_count, 1);
        let mut archive = tar::Archive::new(&buf[..]);
        let paths: Vec<String> = archive
            .entries()
            .expect("entries")
            .map(|e| {
                e.expect("entry")
                    .path()
                    .expect("path")
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        assert!(paths.iter().any(|p| p.contains("/memories/keep/")));
        assert!(
            !paths.iter().any(|p| p.contains("/memories/drop/")),
            "filtered-out memory must not be in the archive; got {paths:?}",
        );
    }
}
