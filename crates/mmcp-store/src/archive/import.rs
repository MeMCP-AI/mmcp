//! Snapshot import: replay a portable archive back into the local
//! store, recreating groups and writing each memory through the same
//! `import_memory` primitive the loose-file import path uses.

use std::collections::BTreeMap;
use std::io::Read;

use mmcp_core::conventions::{MEMORIES_DIR, MEMORY_EXTENSION};
use mmcp_core::id::GroupId;
use mmcp_core::manifest::{GroupManifest, MANIFEST_FILENAME};
use mmcp_core::memory::MemoryFile;
use mmcp_git::{GitBackend, NativeBackend, Rev};
use uuid::Uuid;

use crate::groups::{GroupEntry, GroupIndex};
use crate::home::ResolvedAuthor;
use crate::lock;
use crate::memory::{
    ImportError, import_memory, resolve_group, resolve_memory, write_file_at_path,
};

use super::error::ArchiveError;
use super::manifest::{
    ARCHIVE_FORMAT_VERSION, ARCHIVE_GROUPS_DIR, ARCHIVE_MANIFEST_FILENAME, ArchiveManifest,
};

/// Gzip stream magic; sniffed so import accepts both plain and
/// gzip-compressed archives without the caller declaring which.
const GZIP_MAGIC: [u8; 2] = [0x1f, 0x8b];

/// Hard cap on the total (post-decompression) bytes import reads from
/// an archive, so a gzip bomb or a corrupt length cannot exhaust
/// memory. Generous for archives of text memories.
const MAX_ARCHIVE_BYTES: u64 = 512 * 1024 * 1024;

/// Hard cap on a single archive entry's (post-decompression) bytes.
const MAX_ENTRY_BYTES: u64 = 64 * 1024 * 1024;

/// How an archive should be replayed into the local store.
///
/// `Default`-derived: the common case recreates the original groups
/// by uuid, preserves identities, and skips anything already present.
#[derive(Debug, Clone, Default)]
pub struct ImportArchiveOptions {
    /// Remap every archived memory into this existing local group
    /// instead of recreating the original groups by uuid.
    pub into_group: Option<GroupId>,
    /// Replace a memory whose uuid already exists with differing
    /// content. Off means such a collision is reported, not written.
    pub overwrite: bool,
    /// Mint fresh UUIDs for every imported memory (fork / copy)
    /// rather than preserving the archived identities.
    pub new_ids: bool,
    /// Permit writes into protected existing groups. The surfaces set
    /// this only after confirming the write with the operator.
    pub allow_protected: bool,
    /// When non-empty, import only these archived groups (matched by
    /// uuid or slug). Empty imports every group in the archive.
    pub select_groups: Vec<String>,
    /// When non-empty, import only memories whose slug is in this set.
    /// Empty imports every memory in the selected groups.
    pub select_memory_slugs: Vec<String>,
}

/// A memory the import left untouched because its uuid already exists
/// with differing content and `overwrite` was not set.
#[derive(Debug, Clone)]
pub struct MemoryConflict {
    pub slug: String,
    pub id: Uuid,
}

/// Per-group outcome of an import run.
#[derive(Debug, Clone)]
pub struct GroupImportOutcome {
    /// Group uuid recorded in the archive.
    pub source_group_id: Uuid,
    /// Group uuid the memories actually landed in (differs from
    /// `source_group_id` only under `into_group` remapping).
    pub target_group_id: Uuid,
    /// Group slug recorded in the archive.
    pub slug: String,
    /// True when the target group repo was created by this import.
    pub created_group: bool,
    /// Memories newly written.
    pub created: u32,
    /// Memories replaced in place under `overwrite`.
    pub overwritten: u32,
    /// Memories skipped because an identical copy already existed.
    pub skipped: u32,
    /// Memories left untouched on a differing-content collision.
    pub conflicts: Vec<MemoryConflict>,
}

/// Summary of an import run across every archived group.
#[derive(Debug, Clone, Default)]
pub struct ImportArchiveReport {
    pub groups: Vec<GroupImportOutcome>,
}

/// Read just the table of contents from an archive without importing.
///
/// Surfaces use this to list contents, drive a dry run, and discover
/// which target groups a protected-group confirmation must cover.
pub fn inspect_archive(bytes: &[u8]) -> Result<ArchiveManifest, ArchiveError> {
    let reader = open_reader(bytes);
    let mut archive = tar::Archive::new(reader);
    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.to_string_lossy().into_owned();
        if path == ARCHIVE_MANIFEST_FILENAME {
            let buf = read_capped(&mut entry, &path)?;
            let text = utf8(&path, &buf)?;
            let manifest = ArchiveManifest::from_toml(text)?;
            ensure_supported(&manifest)?;
            return Ok(manifest);
        }
    }
    Err(ArchiveError::MissingManifest)
}

/// One archived group's contents: its identity plus the memory slugs
/// it carries. Surfaces use this to drive an import selection UI.
#[derive(Debug, Clone)]
pub struct ArchiveGroupListing {
    pub group_id: Uuid,
    pub slug: String,
    pub memory_slugs: Vec<String>,
}

/// List every group in an archive together with the memory slugs it
/// carries, so a selection UI can offer group- and memory-level
/// choices before an import runs.
pub fn list_archive(bytes: &[u8]) -> Result<Vec<ArchiveGroupListing>, ArchiveError> {
    let entries = read_entries(bytes)?;
    let manifest = read_toc(&entries)?;
    ensure_supported(&manifest)?;

    let mut listings = Vec::with_capacity(manifest.groups.len());
    for group_meta in &manifest.groups {
        let prefix = format!("{ARCHIVE_GROUPS_DIR}/{}/{MEMORIES_DIR}/", group_meta.group_id);
        let mut memory_slugs: Vec<String> = Vec::new();
        for path in entries.keys() {
            if !path.ends_with(MEMORY_EXTENSION) {
                continue;
            }
            if let Some(remainder) = path.strip_prefix(&prefix)
                && let Some((slug, _)) = remainder.rsplit_once('/')
                && !memory_slugs.iter().any(|s| s == slug)
            {
                memory_slugs.push(slug.to_string());
            }
        }
        memory_slugs.sort();
        listings.push(ArchiveGroupListing {
            group_id: group_meta.group_id,
            slug: group_meta.slug.clone(),
            memory_slugs,
        });
    }
    Ok(listings)
}

/// Replay `bytes` into the local store and report what happened.
pub async fn import_archive(
    backend: &NativeBackend,
    groups: &GroupIndex,
    author: &ResolvedAuthor,
    bytes: &[u8],
    options: &ImportArchiveOptions,
) -> Result<ImportArchiveReport, ArchiveError> {
    let entries = read_entries(bytes)?;
    let manifest = read_toc(&entries)?;
    ensure_supported(&manifest)?;

    // Resolve the single remap target up front when --into is set.
    let into_target = match options.into_group {
        Some(group_id) => Some(groups.get(&group_id).await.ok_or_else(|| {
            ArchiveError::IntoGroupNotFound(group_id.as_uuid().to_string())
        })?),
        None => None,
    };

    // Backstop the protected-group guard before any write so a partial
    // import cannot start against a group the caller has not confirmed.
    protected_precheck(groups, &manifest, into_target.as_ref(), options).await?;

    let mut report = ImportArchiveReport::default();
    for group_meta in &manifest.groups {
        if !group_selected(&options.select_groups, group_meta) {
            continue;
        }
        let outcome = import_one_group(
            backend,
            groups,
            author,
            &entries,
            group_meta.group_id,
            group_meta.slug.clone(),
            into_target.as_ref(),
            options,
        )
        .await?;
        report.groups.push(outcome);
    }
    Ok(report)
}

/// Import every memory belonging to one archived group, recreating or
/// merging the target group as needed.
#[allow(clippy::too_many_arguments)]
async fn import_one_group(
    backend: &NativeBackend,
    groups: &GroupIndex,
    author: &ResolvedAuthor,
    entries: &BTreeMap<String, Vec<u8>>,
    source_group_id: Uuid,
    slug: String,
    into_target: Option<&GroupEntry>,
    options: &ImportArchiveOptions,
) -> Result<GroupImportOutcome, ArchiveError> {
    let (target, created_group) = match into_target {
        Some(target) => (target.clone(), false),
        None => resolve_or_create_target(backend, groups, entries, source_group_id).await?,
    };

    let mut outcome = GroupImportOutcome {
        source_group_id,
        target_group_id: target.handle.group_id,
        slug,
        created_group,
        created: 0,
        overwritten: 0,
        skipped: 0,
        conflicts: Vec::new(),
    };

    let prefix = format!("{ARCHIVE_GROUPS_DIR}/{source_group_id}/{MEMORIES_DIR}/");
    for (path, data) in entries {
        if !path.starts_with(&prefix) || !path.ends_with(MEMORY_EXTENSION) {
            continue;
        }
        let remainder = &path[prefix.len()..];
        let Some((memory_slug, filename)) = remainder.rsplit_once('/') else {
            return Err(ArchiveError::Malformed {
                detail: format!("memory entry `{path}` has no slug directory"),
            });
        };
        if !options.select_memory_slugs.is_empty()
            && !options.select_memory_slugs.iter().any(|s| s == memory_slug)
        {
            continue;
        }
        // The archive filename is `<uuid>.md`; the uuid is the memory's
        // identity for id-less frontmatter (pre-FR-028 / hand-crafted).
        let filename_id = filename
            .strip_suffix(MEMORY_EXTENSION)
            .and_then(|stem| Uuid::parse_str(stem).ok());
        let content = utf8(path, data)?;
        import_one_memory(
            backend,
            &target,
            author,
            memory_slug,
            filename_id,
            content,
            options,
            &mut outcome,
        )
        .await?;
    }

    Ok(outcome)
}

/// Resolve the target group by the archived uuid, creating it from the
/// archived `.mmcp.toml` when it is not yet in the local mirror.
async fn resolve_or_create_target(
    backend: &NativeBackend,
    groups: &GroupIndex,
    entries: &BTreeMap<String, Vec<u8>>,
    source_group_id: Uuid,
) -> Result<(GroupEntry, bool), ArchiveError> {
    match resolve_group(groups, &source_group_id.to_string()).await {
        Ok(existing) => Ok((existing, false)),
        Err(ImportError::GroupNotFound(_)) => {
            let manifest_path = format!("{ARCHIVE_GROUPS_DIR}/{source_group_id}/{MANIFEST_FILENAME}");
            let bytes = entries
                .get(&manifest_path)
                .ok_or(ArchiveError::GroupManifestMissing { group_id: source_group_id })?;
            let text = utf8(&manifest_path, bytes)?;
            let manifest = GroupManifest::from_toml(text).map_err(|source| {
                ArchiveError::GroupManifestParse { group_id: source_group_id, source }
            })?;
            // The repo is created at manifest.group_id; reject an archive
            // whose inner manifest disagrees with its directory uuid so a
            // crafted archive cannot persist a stray repo under a
            // different identity than the operator confirmed against.
            if manifest.group_id.as_uuid() != &source_group_id {
                return Err(ArchiveError::Malformed {
                    detail: format!(
                        "group {source_group_id} manifest declares a different id {}",
                        manifest.group_id.as_uuid()
                    ),
                });
            }
            backend.create_group_repo(&manifest).await?;
            groups.refresh().await?;
            let entry = groups
                .get(&GroupId::from_uuid(source_group_id))
                .await
                .ok_or(ArchiveError::GroupManifestMissing { group_id: source_group_id })?;
            Ok((entry, true))
        }
        Err(other) => Err(other.into()),
    }
}

/// Write one archived memory into `target`, applying the new-ids /
/// overwrite / skip policy and tallying the result on `outcome`.
#[allow(clippy::too_many_arguments)]
async fn import_one_memory(
    backend: &NativeBackend,
    target: &GroupEntry,
    author: &ResolvedAuthor,
    slug: &str,
    filename_id: Option<Uuid>,
    content: &str,
    options: &ImportArchiveOptions,
    outcome: &mut GroupImportOutcome,
) -> Result<(), ArchiveError> {
    // Fork semantics: drop the archived id so a fresh one is minted and
    // the memory always lands as a new sibling.
    if options.new_ids {
        let forked = content_without_id(content)?;
        import_memory(backend, &target.handle, slug, &forked, None, author, false).await?;
        outcome.created += 1;
        return Ok(());
    }

    // Identity-preserving import. The id is the frontmatter id, falling
    // back to the archive filename uuid so id-less files keep their
    // identity and a re-import stays idempotent rather than minting a
    // fresh duplicate every run.
    let Some((id, prepared)) = ensure_id(content, filename_id)? else {
        return Err(ArchiveError::Malformed {
            detail: format!("memory `{slug}` has no id in frontmatter or filename"),
        });
    };

    match resolve_memory(backend, &target.handle, None, Some(id)).await {
        Err(ImportError::MemoryNotFound { .. }) => {
            import_memory(backend, &target.handle, slug, &prepared, None, author, false).await?;
            outcome.created += 1;
        }
        Err(other) => return Err(other.into()),
        Ok(existing) => {
            let existing_bytes = backend
                .read_file(&target.handle, &existing.path, &Rev::Head)
                .await?;
            let existing_text = utf8(&existing.path, &existing_bytes)?;
            if normalize(existing_text)? == normalize(&prepared)? {
                outcome.skipped += 1;
            } else if options.overwrite {
                // Replace at the existing on-disk path so a drifted
                // memory (filename uuid != frontmatter id) is replaced in
                // place rather than duplicated under a canonical name.
                let _guards =
                    lock::acquire_chain(&lock::create_chain(target.handle.group_id)).await;
                write_file_at_path(
                    backend,
                    &target.handle,
                    &existing.path,
                    &prepared,
                    author,
                    existing.addressing_mode,
                    true,
                    None,
                )
                .await?;
                outcome.overwritten += 1;
            } else {
                outcome.conflicts.push(MemoryConflict { slug: existing.slug, id });
            }
        }
    }
    Ok(())
}

/// Error before any write when an archive targets a protected existing
/// group the caller has not opted into writing.
async fn protected_precheck(
    groups: &GroupIndex,
    manifest: &ArchiveManifest,
    into_target: Option<&GroupEntry>,
    options: &ImportArchiveOptions,
) -> Result<(), ArchiveError> {
    if options.allow_protected {
        return Ok(());
    }
    if let Some(target) = into_target {
        if target.manifest.protected {
            return Err(ArchiveError::ProtectedGroup {
                group_id: target.handle.group_id,
                slug: target.manifest.slug.clone(),
            });
        }
        return Ok(());
    }
    for group_meta in &manifest.groups {
        if !group_selected(&options.select_groups, group_meta) {
            continue;
        }
        if let Ok(existing) = resolve_group(groups, &group_meta.group_id.to_string()).await
            && existing.manifest.protected
        {
            return Err(ArchiveError::ProtectedGroup {
                group_id: existing.handle.group_id,
                slug: existing.manifest.slug.clone(),
            });
        }
    }
    Ok(())
}

/// Whether an archived group is in scope for this import. An empty
/// selection imports every group; otherwise a group matches by its
/// uuid string or its slug.
fn group_selected(select: &[String], group_meta: &super::manifest::ArchivedGroupMeta) -> bool {
    select.is_empty()
        || select
            .iter()
            .any(|s| s == &group_meta.group_id.to_string() || s == &group_meta.slug)
}

/// Reject an archive whose layout version is newer than this build.
fn ensure_supported(manifest: &ArchiveManifest) -> Result<(), ArchiveError> {
    if manifest.format_version > ARCHIVE_FORMAT_VERSION {
        return Err(ArchiveError::UnsupportedFormatVersion {
            found: manifest.format_version,
            supported: ARCHIVE_FORMAT_VERSION,
        });
    }
    Ok(())
}

/// Read every tar entry into a path-keyed map, transparently
/// decompressing a gzip stream. Rejects duplicate paths so a crafted
/// archive cannot shadow the table of contents the protected-group
/// confirmation was driven from.
fn read_entries(bytes: &[u8]) -> Result<BTreeMap<String, Vec<u8>>, ArchiveError> {
    let reader = open_reader(bytes);
    let mut archive = tar::Archive::new(reader);
    let mut map = BTreeMap::new();
    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.to_string_lossy().into_owned();
        let buf = read_capped(&mut entry, &path)?;
        if map.insert(path.clone(), buf).is_some() {
            return Err(ArchiveError::Malformed {
                detail: format!("duplicate archive entry `{path}`"),
            });
        }
    }
    Ok(map)
}

/// Read one entry's bytes, rejecting anything past the per-entry cap.
fn read_capped<R: Read>(entry: &mut R, path: &str) -> Result<Vec<u8>, ArchiveError> {
    let mut buf = Vec::new();
    entry.take(MAX_ENTRY_BYTES + 1).read_to_end(&mut buf)?;
    if buf.len() as u64 > MAX_ENTRY_BYTES {
        return Err(ArchiveError::Malformed {
            detail: format!("archive entry `{path}` exceeds the per-entry size limit"),
        });
    }
    Ok(buf)
}

/// Look up and parse the `archive.toml` table of contents.
fn read_toc(entries: &BTreeMap<String, Vec<u8>>) -> Result<ArchiveManifest, ArchiveError> {
    let bytes = entries
        .get(ARCHIVE_MANIFEST_FILENAME)
        .ok_or(ArchiveError::MissingManifest)?;
    let text = utf8(ARCHIVE_MANIFEST_FILENAME, bytes)?;
    Ok(ArchiveManifest::from_toml(text)?)
}

/// Wrap the raw bytes in a (multi-member) gzip decoder when the gzip
/// magic is present, then cap the total decompressed bytes so a bomb
/// cannot exhaust memory.
fn open_reader(bytes: &[u8]) -> Box<dyn Read + '_> {
    let raw: Box<dyn Read + '_> =
        if bytes.len() >= GZIP_MAGIC.len() && bytes[..GZIP_MAGIC.len()] == GZIP_MAGIC {
            Box::new(flate2::read::MultiGzDecoder::new(bytes))
        } else {
            Box::new(bytes)
        };
    Box::new(raw.take(MAX_ARCHIVE_BYTES))
}

/// Decode an archive entry's bytes as UTF-8, attributing failures to
/// the entry path.
fn utf8<'a>(path: &str, bytes: &'a [u8]) -> Result<&'a str, ArchiveError> {
    std::str::from_utf8(bytes).map_err(|source| ArchiveError::NotUtf8 {
        path: path.to_string(),
        source,
    })
}

/// Re-render a memory document with its frontmatter id removed.
fn content_without_id(content: &str) -> Result<String, ArchiveError> {
    let mut parsed = MemoryFile::parse(content).map_err(ImportError::Parse)?;
    parsed.frontmatter.id = None;
    let rendered = parsed.to_string().map_err(ImportError::Parse)?;
    Ok(rendered)
}

/// Resolve a memory's effective id (frontmatter id, else the archive
/// filename uuid) and return the content guaranteed to carry it. Yields
/// `None` only when the memory has no id in either place — an archive
/// whose identity cannot be preserved.
fn ensure_id(content: &str, fallback: Option<Uuid>) -> Result<Option<(Uuid, String)>, ArchiveError> {
    let mut parsed = MemoryFile::parse(content).map_err(ImportError::Parse)?;
    if let Some(id) = parsed.frontmatter.id {
        return Ok(Some((id, content.to_string())));
    }
    let Some(id) = fallback else {
        return Ok(None);
    };
    parsed.frontmatter.id = Some(id);
    let rendered = parsed.to_string().map_err(ImportError::Parse)?;
    Ok(Some((id, rendered)))
}

/// Re-render a memory document in canonical form so two copies compare
/// equal regardless of incidental formatting differences.
fn normalize(content: &str) -> Result<String, ArchiveError> {
    let parsed = MemoryFile::parse(content).map_err(ImportError::Parse)?;
    let rendered = parsed.to_string().map_err(ImportError::Parse)?;
    Ok(rendered)
}

#[cfg(test)]
mod tests {
    use super::super::manifest::{ArchiveMode, ArchivedGroupMeta};
    use super::*;
    use crate::testing::ScratchHome;
    use crate::{ExportOptions, export_archive, list_all_memory_files};

    /// Build a minimal memory document with an explicit id so tests
    /// control the round-trip identity.
    fn memory_doc(id: Uuid, body: &str) -> String {
        format!(
            "+++\nid = \"{id}\"\nname = \"note\"\ndescription = \"d\"\nkind = \"reference\"\n+++\n{body}\n"
        )
    }

    async fn export_group(home: &ScratchHome, group: GroupId) -> Vec<u8> {
        let entry = home.groups().get(&group).await.expect("group entry");
        let mut buf = Vec::new();
        export_archive(home.backend(), &[entry], &ExportOptions::default(), &mut buf)
            .await
            .expect("export");
        buf
    }

    /// Hand-build an archive carrying one memory whose frontmatter body
    /// is supplied verbatim — used to construct id-less inputs the
    /// store's own export path never produces.
    fn build_archive(
        group_id: Uuid,
        group_manifest: &GroupManifest,
        slug: &str,
        memory_id: Uuid,
        memory_body: &str,
    ) -> Vec<u8> {
        let toc = ArchiveManifest {
            format_version: ARCHIVE_FORMAT_VERSION,
            mode: ArchiveMode::Snapshot,
            mmcp_version: "test".to_string(),
            created_at: "2026-01-01T00:00:00Z".to_string(),
            groups: vec![ArchivedGroupMeta {
                group_id,
                slug: group_manifest.slug.clone(),
                display_name: None,
                memory_count: 1,
            }],
        }
        .to_toml()
        .expect("toc");
        let group_toml = group_manifest.to_toml().expect("group manifest");

        let mut buf = Vec::new();
        {
            let mut builder = tar::Builder::new(&mut buf);
            append_entry(&mut builder, ARCHIVE_MANIFEST_FILENAME, toc.as_bytes());
            append_entry(
                &mut builder,
                &format!("{ARCHIVE_GROUPS_DIR}/{group_id}/{MANIFEST_FILENAME}"),
                group_toml.as_bytes(),
            );
            append_entry(
                &mut builder,
                &format!("{ARCHIVE_GROUPS_DIR}/{group_id}/{MEMORIES_DIR}/{slug}/{memory_id}.md"),
                memory_body.as_bytes(),
            );
            builder.finish().expect("finish tar");
        }
        buf
    }

    fn append_entry(builder: &mut tar::Builder<&mut Vec<u8>>, path: &str, data: &[u8]) {
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::Regular);
        header.set_size(data.len() as u64);
        header.set_mode(0o644);
        builder.append_data(&mut header, path, data).expect("append");
    }

    #[test]
    fn ensure_id_falls_back_to_filename_then_errors_when_absent() {
        let filename_id = Uuid::now_v7();
        let no_id = "+++\nname = \"n\"\ndescription = \"d\"\nkind = \"reference\"\n+++\nBody.\n";
        let (effective, prepared) = ensure_id(no_id, Some(filename_id))
            .expect("ensure")
            .expect("has id");
        assert_eq!(effective, filename_id);
        assert!(
            prepared.contains(&filename_id.to_string()),
            "filename id must be injected into frontmatter",
        );

        // An explicit frontmatter id wins and the content is unchanged.
        let explicit = Uuid::now_v7();
        let doc = memory_doc(explicit, "Body.");
        let (effective2, prepared2) = ensure_id(&doc, Some(Uuid::now_v7()))
            .expect("ensure")
            .expect("has id");
        assert_eq!(effective2, explicit);
        assert_eq!(prepared2, doc);

        // No id anywhere is unresolvable.
        assert!(ensure_id(no_id, None).expect("ensure").is_none());
    }

    #[tokio::test]
    async fn round_trip_recreates_group_and_preserves_identity() {
        let id = Uuid::now_v7();
        let src = ScratchHome::new().await.expect("src home");
        let seeded = src.seed_group("origin").await.expect("seed");
        let entry = src.groups().get(&seeded.group_id).await.expect("entry");
        import_memory(
            src.backend(),
            &entry.handle,
            "note",
            &memory_doc(id, "Body one."),
            None,
            src.author(),
            false,
        )
        .await
        .expect("seed memory");
        let buf = export_group(&src, seeded.group_id).await;

        let dst = ScratchHome::new().await.expect("dst home");
        let report = import_archive(
            dst.backend(),
            dst.groups(),
            dst.author(),
            &buf,
            &ImportArchiveOptions::default(),
        )
        .await
        .expect("import");

        assert_eq!(report.groups.len(), 1);
        let g = &report.groups[0];
        assert!(g.created_group, "group should be recreated");
        assert_eq!(g.created, 1);
        assert_eq!(g.target_group_id, *seeded.group_id.as_uuid());

        let dst_entry = dst
            .groups()
            .get(&seeded.group_id)
            .await
            .expect("group recreated in dst");
        let files = list_all_memory_files(dst.backend(), &dst_entry.handle, &Rev::Head)
            .await
            .expect("list");
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].id, id);
        assert_eq!(files[0].slug, "note");
    }

    #[tokio::test]
    async fn reimport_skips_identical_memories() {
        let id = Uuid::now_v7();
        let src = ScratchHome::new().await.expect("src home");
        let seeded = src.seed_group("origin").await.expect("seed");
        let entry = src.groups().get(&seeded.group_id).await.expect("entry");
        import_memory(
            src.backend(),
            &entry.handle,
            "note",
            &memory_doc(id, "Body one."),
            None,
            src.author(),
            false,
        )
        .await
        .expect("seed memory");
        let buf = export_group(&src, seeded.group_id).await;

        let dst = ScratchHome::new().await.expect("dst home");
        let first = import_archive(
            dst.backend(),
            dst.groups(),
            dst.author(),
            &buf,
            &ImportArchiveOptions::default(),
        )
        .await
        .expect("first import");
        assert_eq!(first.groups[0].created, 1);

        let second = import_archive(
            dst.backend(),
            dst.groups(),
            dst.author(),
            &buf,
            &ImportArchiveOptions::default(),
        )
        .await
        .expect("second import");
        assert_eq!(second.groups[0].created, 0);
        assert_eq!(second.groups[0].skipped, 1);
        assert!(!second.groups[0].created_group);
    }

    #[tokio::test]
    async fn idless_memory_keeps_filename_id_and_is_idempotent() {
        let home = ScratchHome::new().await.expect("home");
        let seeded = home.seed_group("origin").await.expect("seed");
        let group_id = *seeded.group_id.as_uuid();
        let memory_id = Uuid::now_v7();
        // Frontmatter without an id; the identity lives only in the
        // archive filename `<memory_id>.md`.
        let body = "+++\nname = \"n\"\ndescription = \"d\"\nkind = \"reference\"\n+++\nBody.\n";
        let buf = build_archive(group_id, &seeded.manifest, "note", memory_id, body);

        let first = import_archive(
            home.backend(),
            home.groups(),
            home.author(),
            &buf,
            &ImportArchiveOptions::default(),
        )
        .await
        .expect("first import");
        assert_eq!(first.groups[0].created, 1);

        let entry = home.groups().get(&seeded.group_id).await.expect("entry");
        let files = list_all_memory_files(home.backend(), &entry.handle, &Rev::Head)
            .await
            .expect("list");
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].id, memory_id, "filename id preserved, not re-minted");

        let second = import_archive(
            home.backend(),
            home.groups(),
            home.author(),
            &buf,
            &ImportArchiveOptions::default(),
        )
        .await
        .expect("second import");
        assert_eq!(second.groups[0].created, 0);
        assert_eq!(second.groups[0].skipped, 1);
        let files = list_all_memory_files(home.backend(), &entry.handle, &Rev::Head)
            .await
            .expect("list");
        assert_eq!(files.len(), 1, "no duplicate sibling minted");
    }

    #[tokio::test]
    async fn rejects_archive_with_mismatched_inner_group_manifest() {
        let src = ScratchHome::new().await.expect("src home");
        let seeded = src.seed_group("origin").await.expect("seed");
        let entry = src.groups().get(&seeded.group_id).await.expect("entry");
        import_memory(
            src.backend(),
            &entry.handle,
            "note",
            &memory_doc(Uuid::now_v7(), "Body."),
            None,
            src.author(),
            false,
        )
        .await
        .expect("seed memory");
        let mut buf = export_group(&src, seeded.group_id).await;

        // Corrupt the inner manifest so its declared group_id differs
        // from the archive directory uuid.
        let original = seeded.manifest.to_toml().expect("manifest");
        let tampered = original.replace(
            seeded.group_id.as_uuid().to_string().as_str(),
            Uuid::now_v7().to_string().as_str(),
        );
        assert_ne!(original, tampered, "manifest must contain the group id");
        buf = rewrite_group_manifest(&buf, *seeded.group_id.as_uuid(), &tampered);

        let dst = ScratchHome::new().await.expect("dst home");
        let err = import_archive(
            dst.backend(),
            dst.groups(),
            dst.author(),
            &buf,
            &ImportArchiveOptions::default(),
        )
        .await
        .expect_err("mismatched manifest must be rejected");
        assert!(matches!(err, ArchiveError::Malformed { .. }), "got {err:?}");
    }

    /// Rebuild an archive replacing one group's `.mmcp.toml` bytes.
    fn rewrite_group_manifest(bytes: &[u8], group_id: Uuid, new_manifest: &str) -> Vec<u8> {
        let manifest_path = format!("{ARCHIVE_GROUPS_DIR}/{group_id}/{MANIFEST_FILENAME}");
        let mut out = Vec::new();
        {
            let mut builder = tar::Builder::new(&mut out);
            let mut archive = tar::Archive::new(bytes);
            for entry in archive.entries().expect("entries") {
                let mut entry = entry.expect("entry");
                let path = entry.path().expect("path").to_string_lossy().into_owned();
                let mut buf = Vec::new();
                entry.read_to_end(&mut buf).expect("read");
                let data = if path == manifest_path {
                    new_manifest.as_bytes().to_vec()
                } else {
                    buf
                };
                append_entry(&mut builder, &path, &data);
            }
            builder.finish().expect("finish");
        }
        out
    }

    #[tokio::test]
    async fn differing_content_conflicts_then_overwrites() {
        let id = Uuid::now_v7();
        let home = ScratchHome::new().await.expect("home");
        let seeded = home.seed_group("origin").await.expect("seed");
        let entry = home.groups().get(&seeded.group_id).await.expect("entry");
        import_memory(
            home.backend(),
            &entry.handle,
            "note",
            &memory_doc(id, "Body one."),
            None,
            home.author(),
            false,
        )
        .await
        .expect("seed memory");
        let buf = export_group(&home, seeded.group_id).await;

        // Mutate the same memory in place so the archive copy now
        // differs from what is on disk.
        import_memory(
            home.backend(),
            &entry.handle,
            "note",
            &memory_doc(id, "Body two."),
            None,
            home.author(),
            true,
        )
        .await
        .expect("mutate memory");

        let conflict = import_archive(
            home.backend(),
            home.groups(),
            home.author(),
            &buf,
            &ImportArchiveOptions::default(),
        )
        .await
        .expect("conflict import");
        assert_eq!(conflict.groups[0].created, 0);
        assert_eq!(conflict.groups[0].skipped, 0);
        assert_eq!(conflict.groups[0].conflicts.len(), 1);
        assert_eq!(conflict.groups[0].conflicts[0].id, id);

        let overwritten = import_archive(
            home.backend(),
            home.groups(),
            home.author(),
            &buf,
            &ImportArchiveOptions {
                overwrite: true,
                ..Default::default()
            },
        )
        .await
        .expect("overwrite import");
        assert_eq!(overwritten.groups[0].overwritten, 1);

        let resolved = resolve_memory(home.backend(), &entry.handle, None, Some(id))
            .await
            .expect("resolve");
        let restored = home
            .backend()
            .read_file(&entry.handle, &resolved.path, &Rev::Head)
            .await
            .expect("read restored");
        let restored = utf8(&resolved.path, &restored).expect("utf8").to_string();
        assert!(restored.contains("Body one."), "overwrite restored v1");
    }

    #[tokio::test]
    async fn new_ids_forks_into_fresh_identities() {
        let id = Uuid::now_v7();
        let home = ScratchHome::new().await.expect("home");
        let seeded = home.seed_group("origin").await.expect("seed");
        let entry = home.groups().get(&seeded.group_id).await.expect("entry");
        import_memory(
            home.backend(),
            &entry.handle,
            "note",
            &memory_doc(id, "Body one."),
            None,
            home.author(),
            false,
        )
        .await
        .expect("seed memory");
        let buf = export_group(&home, seeded.group_id).await;

        let report = import_archive(
            home.backend(),
            home.groups(),
            home.author(),
            &buf,
            &ImportArchiveOptions {
                new_ids: true,
                ..Default::default()
            },
        )
        .await
        .expect("fork import");
        assert_eq!(report.groups[0].created, 1);

        let files = list_all_memory_files(home.backend(), &entry.handle, &Rev::Head)
            .await
            .expect("list");
        assert_eq!(files.len(), 2, "fork adds a sibling: {files:?}");
        assert!(files.iter().any(|f| f.id == id), "original survives");
        assert!(files.iter().any(|f| f.id != id), "fork has a fresh id");
    }

    #[tokio::test]
    async fn into_remaps_memories_into_target_group() {
        let id = Uuid::now_v7();
        let src = ScratchHome::new().await.expect("src home");
        let seeded = src.seed_group("origin").await.expect("seed");
        let entry = src.groups().get(&seeded.group_id).await.expect("entry");
        import_memory(
            src.backend(),
            &entry.handle,
            "note",
            &memory_doc(id, "Body one."),
            None,
            src.author(),
            false,
        )
        .await
        .expect("seed memory");
        let buf = export_group(&src, seeded.group_id).await;

        let dst = ScratchHome::new().await.expect("dst home");
        let target = dst.seed_group("target").await.expect("seed target");
        let report = import_archive(
            dst.backend(),
            dst.groups(),
            dst.author(),
            &buf,
            &ImportArchiveOptions {
                into_group: Some(target.group_id),
                ..Default::default()
            },
        )
        .await
        .expect("remap import");

        assert_eq!(report.groups.len(), 1);
        let g = &report.groups[0];
        assert_eq!(g.source_group_id, *seeded.group_id.as_uuid());
        assert_eq!(g.target_group_id, *target.group_id.as_uuid());
        assert!(!g.created_group);
        assert_eq!(g.created, 1);

        let target_entry = dst.groups().get(&target.group_id).await.expect("target");
        let files = list_all_memory_files(dst.backend(), &target_entry.handle, &Rev::Head)
            .await
            .expect("list");
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].id, id);
        assert!(
            dst.groups().get(&seeded.group_id).await.is_none(),
            "source group must not be recreated under --into",
        );
    }

    #[tokio::test]
    async fn select_memory_slugs_imports_only_the_chosen_memory() {
        let src = ScratchHome::new().await.expect("src");
        let seeded = src.seed_group("origin").await.expect("seed");
        let entry = src.groups().get(&seeded.group_id).await.expect("entry");
        for (slug, body) in [("keep", "K"), ("drop", "D")] {
            import_memory(
                src.backend(),
                &entry.handle,
                slug,
                &memory_doc(Uuid::now_v7(), body),
                None,
                src.author(),
                false,
            )
            .await
            .expect("seed memory");
        }
        let buf = export_group(&src, seeded.group_id).await;

        let dst = ScratchHome::new().await.expect("dst");
        let report = import_archive(
            dst.backend(),
            dst.groups(),
            dst.author(),
            &buf,
            &ImportArchiveOptions {
                select_memory_slugs: vec!["keep".to_string()],
                ..Default::default()
            },
        )
        .await
        .expect("import");
        assert_eq!(report.groups[0].created, 1);

        let dst_entry = dst.groups().get(&seeded.group_id).await.expect("group");
        let files = list_all_memory_files(dst.backend(), &dst_entry.handle, &Rev::Head)
            .await
            .expect("list");
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].slug, "keep");
    }

    #[tokio::test]
    async fn select_groups_imports_only_the_chosen_group() {
        let src = ScratchHome::new().await.expect("src");
        let alpha = src.seed_group("alpha").await.expect("alpha");
        let beta = src.seed_group("beta").await.expect("beta");
        let alpha_entry = src.groups().get(&alpha.group_id).await.expect("alpha entry");
        let beta_entry = src.groups().get(&beta.group_id).await.expect("beta entry");
        import_memory(
            src.backend(),
            &alpha_entry.handle,
            "a",
            &memory_doc(Uuid::now_v7(), "A"),
            None,
            src.author(),
            false,
        )
        .await
        .expect("a");
        import_memory(
            src.backend(),
            &beta_entry.handle,
            "b",
            &memory_doc(Uuid::now_v7(), "B"),
            None,
            src.author(),
            false,
        )
        .await
        .expect("b");
        let mut buf = Vec::new();
        export_archive(
            src.backend(),
            &[alpha_entry, beta_entry],
            &ExportOptions::default(),
            &mut buf,
        )
        .await
        .expect("export both");

        let dst = ScratchHome::new().await.expect("dst");
        let report = import_archive(
            dst.backend(),
            dst.groups(),
            dst.author(),
            &buf,
            &ImportArchiveOptions {
                select_groups: vec![beta.group_id.as_uuid().to_string()],
                ..Default::default()
            },
        )
        .await
        .expect("import");
        assert_eq!(report.groups.len(), 1);
        assert_eq!(report.groups[0].source_group_id, *beta.group_id.as_uuid());
        assert!(
            dst.groups().get(&alpha.group_id).await.is_none(),
            "alpha must not be imported",
        );
        assert!(
            dst.groups().get(&beta.group_id).await.is_some(),
            "beta must be imported",
        );
    }

    #[tokio::test]
    async fn list_archive_reports_groups_and_memory_slugs() {
        let src = ScratchHome::new().await.expect("src");
        let seeded = src.seed_group("origin").await.expect("seed");
        let entry = src.groups().get(&seeded.group_id).await.expect("entry");
        for slug in ["alpha", "beta"] {
            import_memory(
                src.backend(),
                &entry.handle,
                slug,
                &memory_doc(Uuid::now_v7(), "B"),
                None,
                src.author(),
                false,
            )
            .await
            .expect("seed");
        }
        let buf = export_group(&src, seeded.group_id).await;

        let listing = list_archive(&buf).expect("listing");
        assert_eq!(listing.len(), 1);
        assert_eq!(listing[0].group_id, *seeded.group_id.as_uuid());
        assert_eq!(listing[0].slug, "origin");
        assert_eq!(
            listing[0].memory_slugs,
            vec!["alpha".to_string(), "beta".to_string()]
        );
    }
}
