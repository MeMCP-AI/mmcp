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
use crate::memory::{ImportError, list_all_memory_files};

use super::error::ArchiveError;
use super::filter::MemoryFilter;
use super::manifest::{
    ARCHIVE_FORMAT_VERSION, ARCHIVE_GROUPS_DIR, ARCHIVE_MANIFEST_FILENAME, ArchiveManifest,
    ArchiveMode, ArchivedGroupMeta,
};

/// Unix mode bits stamped on every archive entry: owner read/write,
/// group/other read. Archived memories are data, never executable.
const ARCHIVE_ENTRY_MODE: u32 = 0o644;

/// Fixed modification time (epoch seconds) on every entry so two
/// exports of identical store content differ only by the manifest's
/// own `created_at` stamp, not by per-file mtimes.
const ARCHIVE_ENTRY_MTIME: u64 = 0;

/// Knobs for an export run. `Default`-derived so call sites set only
/// the toggles they care about.
#[derive(Debug, Clone, Default)]
pub struct ExportOptions {
    /// gzip the tar stream (pure-Rust flate2). Off means a plain tar.
    pub gzip: bool,
    /// Facet filter narrowing which memories are packed. Empty matches
    /// every memory in the selected groups.
    pub filter: MemoryFilter,
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

        let manifest_bytes = backend
            .read_file(&group.handle, MANIFEST_FILENAME, &Rev::Head)
            .await?;
        entries.push((format!("{base}/{MANIFEST_FILENAME}"), manifest_bytes.to_vec()));

        let files = list_all_memory_files(backend, &group.handle, &Rev::Head).await?;
        let mut packed: u32 = 0;
        for file in &files {
            let bytes = backend
                .read_file(&group.handle, &file.path, &Rev::Head)
                .await?;
            if !options.filter.is_empty() {
                let text = std::str::from_utf8(&bytes).map_err(|source| ArchiveError::NotUtf8 {
                    path: file.path.clone(),
                    source,
                })?;
                let parsed = MemoryFile::parse(text).map_err(ImportError::Parse)?;
                if !options.filter.matches(&file.slug, &parsed.frontmatter, &parsed.body) {
                    continue;
                }
            }
            entries.push((format!("{base}/{}", file.path), bytes.to_vec()));
            packed = packed.saturating_add(1);
        }

        group_metas.push(ArchivedGroupMeta {
            group_id,
            slug: group.manifest.slug.clone(),
            display_name: group.manifest.display_name.clone(),
            memory_count: packed,
        });
    }

    let manifest = ArchiveManifest {
        format_version: ARCHIVE_FORMAT_VERSION,
        mode: ArchiveMode::Snapshot,
        mmcp_version: env!("CARGO_PKG_VERSION").to_string(),
        created_at: jiff::Timestamp::now().to_string(),
        groups: group_metas,
    };

    write_archive(&manifest, &entries, writer, options.gzip)?;
    Ok(manifest)
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
        assert!(paths.iter().any(|p| *p == format!("groups/{gid}/.mmcp.toml")));
        assert!(
            paths.iter().any(|p| p
                .starts_with(&format!("groups/{gid}/memories/note/"))
                && p.ends_with(".md")),
            "memory entry missing; got {paths:?}",
        );

        let parsed = ArchiveManifest::from_toml(&toc).expect("parse toc");
        assert_eq!(parsed.format_version, ARCHIVE_FORMAT_VERSION);
        assert_eq!(parsed.total_memory_count(), 1);
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
