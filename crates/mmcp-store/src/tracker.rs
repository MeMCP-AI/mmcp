//! Shared tracker-layer machinery.
//!
//! `features` and `issues` are structurally near-identical CRUD surfaces,
//! over two distinct memory kinds (`Feature` and `Issue`),
//! that deliberately keep distinct status vocabularies: two systems, two purposes.
//!
//! What *is* shared is the surrounding plumbing:
//! the per-group ticket counter, cross-reference composition, slug-rename git planning,
//! and the listing/notes-channel shape that decides which records survive a filter,
//! and how a corrupt-on-disk memory gets reported instead of silently dropped.
//!
//! The helper lives in its own concern-named module so neither tracker surface owns it.

use std::future::Future;

use mmcp_core::conventions::{MEMORIES_DIR, MEMORY_EXTENSION, memory_path};
use mmcp_core::id::MemoryId;
use mmcp_core::memory::{MemoryFile, MemoryFrontmatter, MemoryRef, Status};
use mmcp_git::{GitBackend, GitError, NativeBackend, Rev};
use uuid::Uuid;

use crate::diagnostics::Finding;
use crate::groups::GroupEntry;
use crate::memory::{ImportError, list_memory_slug_dirs, validate_memory_slug};

/// Batched-read outcome shape [`NativeBackend::read_files`] returns.
/// Spelled out locally because its own alias sits in a private module of `mmcp-git`.
type BatchOutcome = Vec<(String, Result<bytes::Bytes, GitError>)>;

/// Compute the next ticket number for the group.
///
/// Reads the frontmatter of every memory under `memories/`,
/// looks at `feature.number` and `issue.number`,
/// and returns one more than the maximum observed value.
/// Returns `1` for an empty group.
/// Nested slug paths are walked recursively via [`list_memory_slug_dirs`].
///
/// Deliberately not `read_all_slug_files`: that helper errors on an ambiguous slug.
/// This counter folds every file of an ambiguous slug; skipping one could reissue an allocated number.
///
/// Errors only on a hard list / read failure on the underlying git tree;
/// per-memory parse errors are ignored so a single malformed file does not stall the counter.
pub async fn next_ticket_number(
    backend: &NativeBackend,
    entry: &GroupEntry,
) -> Result<u32, ImportError> {
    let rev = Rev::head();
    let slug_dirs = list_memory_slug_dirs(backend, &entry.handle, &rev)
        .await
        .map_err(ImportError::Git)?;

    let paths: Vec<String> = slug_dirs
        .iter()
        .flat_map(|slug_dir| {
            slug_dir
                .filenames
                .iter()
                .map(|filename| format!("{}/{filename}", slug_dir.dir))
        })
        .collect();

    // One resolve of the commit and root tree, reused for every path below,
    // instead of once per file like a `read_file`-per-file loop would pay.
    let max = max_ticket_number_over(paths, |paths| {
        backend.read_files(&entry.handle, paths, &rev)
    })
    .await?;
    // `max` is accumulated from frontmatter `meta.number`, a value any client can write into a
    // memory file without an upper bound enforced at write time. A plain `max + 1` would wrap to
    // `0` in a release build (overflow-checks off) or panic in debug, either way reissuing an
    // already-allocated ticket number instead of surfacing the exhausted counter.
    max.checked_add(1).ok_or(ImportError::TicketCounterOverflow)
}

/// Fold `feature.number` / `issue.number` out of every path in `paths`, via one call to `read_batch`.
/// `read_batch` is the batched-read seam: production passes [`NativeBackend::read_files`] directly.
/// A test can pass a call-counting stub instead, to prove this folds without a per-path round trip.
/// A per-path read or parse failure is skipped, never aborts the fold.
/// A hard failure of `read_batch` itself (the whole batch call) is the only error this raises.
async fn max_ticket_number_over<F, Fut>(
    paths: Vec<String>,
    read_batch: F,
) -> Result<u32, ImportError>
where
    F: FnOnce(Vec<String>) -> Fut,
    Fut: Future<Output = Result<BatchOutcome, GitError>>,
{
    let batch = read_batch(paths).await.map_err(ImportError::Git)?;
    let mut max = 0u32;
    for (_path, outcome) in batch {
        let Ok(bytes) = outcome else { continue };
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
    Ok(max)
}

/// Compose the merged cross-reference list for an update.
/// Removals apply first, then each addition replaces any existing ref to the same target.
/// Shared between `update_feature` and `update_issue`.
pub(crate) fn compose_refs(
    current: Vec<MemoryRef>,
    remove: Option<&[Uuid]>,
    add: Option<&[MemoryRef]>,
) -> Vec<MemoryRef> {
    let mut out = current;
    if let Some(remove) = remove {
        out.retain(|r| !remove.contains(&r.target));
    }
    if let Some(add) = add {
        for new in add {
            out.retain(|r| r.target != new.target);
            out.push(new.clone());
        }
    }
    out
}

/// Carry forward every frontmatter field an update does not itself own.
///
/// `updated` supplies `name`, `description`, and its own tracker metadata block.
/// `current` supplies everything else: `tags`, `mandatory`, `version`, `bump_intent`, `source`.
/// `current` also supplies `kind` and any `feature`/`issue`/`milestone` block `updated` left unset.
/// A hybrid record keeps its other tracker block through this carry-forward (see `kind.rs`).
///
/// `refs` is `Some` to use a caller-composed cross-reference list.
/// `None` carries `current.refs` forward unchanged.
pub(crate) fn carry_forward_frontmatter(
    updated: MemoryFrontmatter,
    current: &MemoryFrontmatter,
    refs: Option<Vec<MemoryRef>>,
) -> MemoryFrontmatter {
    let mut updated = updated
        .with_tags(current.tags.clone())
        .with_mandatory(current.mandatory)
        .with_version(current.version.clone())
        .with_bump_intent(current.bump_intent)
        .with_source(current.source)
        .with_refs(refs.unwrap_or_else(|| current.refs.clone()));
    updated.kind = current.kind;
    updated.feature = updated.feature.or_else(|| current.feature.clone());
    updated.issue = updated.issue.or_else(|| current.issue.clone());
    updated.milestone = updated.milestone.or_else(|| current.milestone.clone());
    updated
}

/// Decide whether a tracker record carrying `status` survives a listing filter.
/// Shared precedence between `list_features` and `list_issues`:
///
/// 1. `status_filter = Some(x)` wins over `show_all`: an explicit selector always includes a matching record.
/// 2. `status_filter = None`, `show_all = true`: every record.
/// 3. `status_filter = None`, `show_all = false`:
///    hide whatever the status's own [`Status::is_default_hidden`] marks terminal-ish,
///    so each tracker kind declares its own default visibility once instead of every call site re-deciding it.
pub(crate) fn listing_keeps_status<S: Status>(
    status: S,
    status_filter: Option<S>,
    show_all: bool,
) -> bool {
    match status_filter {
        Some(want) => status == want,
        None if show_all => true,
        None => !status.is_default_hidden(),
    }
}

/// Extract a tracker's metadata block from a parsed memory's frontmatter, erroring when it is absent.
///
/// Shared block-presence gating between `features::record_from_file`, `issues::record_from_file`,
/// and `milestones::crud::record_from_file` (and the matching `reject_foreign` predicates their
/// `rename_*` siblings pass to [`plan_slug_rename`]): a memory belongs to a tracker surface exactly
/// when it carries that tracker's own frontmatter block, never by `frontmatter.kind` alone.
/// `kind.rs` documents the hybrid model: a memory may carry both a `[feature]` and an `[issue]`
/// block at once, and [`next_ticket_number`] above already reads both blocks off one [`MemoryFile`]
/// to compute the shared counter, so kind-only gating would reject a real hybrid a block-presence
/// check accepts correctly.
///
/// `not_found` builds the caller's own typed "missing block" error from `(slug, kind)`,
/// so each tracker keeps its distinct `NotAFeature` / `NotAnIssue` / `NotAMilestone` variant.
pub(crate) fn require_block<T, E>(
    slug: &str,
    kind: &str,
    block: Option<T>,
    not_found: impl FnOnce(String, String) -> E,
) -> Result<T, E> {
    block.ok_or_else(|| not_found(slug.to_string(), kind.to_string()))
}

/// Build the `frontmatter_parse_failed` [`Finding`] both `list_features` and `list_issues` emit,
/// for a memory whose frontmatter failed to parse,
/// so a corrupt-on-disk tracker memory is reported instead of silently vanishing from the listing.
///
/// `pub` (not `pub(crate)`): `mmcp-client`'s generic `list_memories`/`search_memories` tools reuse this shape,
/// for the same parse-failure-fabrication pattern outside the tracker kinds,
/// so every caller across the workspace reports one `frontmatter_parse_failed` code,
/// with one message format instead of drifting per crate.
pub fn parse_failed_finding(
    group: &str,
    slug: &str,
    err: &mmcp_core::memory::MemoryParseError,
) -> Finding {
    Finding {
        group: group.to_string(),
        slug: Some(slug.to_string()),
        severity: "error",
        code: "frontmatter_parse_failed",
        message: format!("frontmatter parse failed: {err}"),
    }
}

/// Build the `memory_not_utf8` [`Finding`] every non-UTF8 memory-file
/// read path emits, for a memory file whose bytes did not decode as
/// UTF-8, so a corrupt-on-disk file is reported instead of silently
/// dropped or aborting the whole listing.
/// The message carries the underlying `Utf8Error`.
///
/// `pub` (not `pub(crate)`): `mmcp-client`'s `list_memories` MCP tool
/// and its `resolve_subscribed_reads` subscription-read path both hit
/// this same failure mode outside the tracker kinds, so every caller
/// across the workspace builds one `memory_not_utf8` finding with one
/// message format instead of drifting per call site.
pub fn not_utf8_finding(group: &str, slug: &str, err: &std::str::Utf8Error) -> Finding {
    Finding {
        group: group.to_string(),
        slug: Some(slug.to_string()),
        severity: "error",
        code: "memory_not_utf8",
        message: format!("memory file is not valid UTF-8: {err}"),
    }
}

/// Git move-list produced by [`plan_slug_rename`], ready for `CommitSpec::mmcp_commit`.
pub(crate) type PlannedRename = Vec<(String, Option<Vec<u8>>)>;

/// Walk every memory file under `old_slug`'s two-level directory,
/// parse each, and stage a git move to `new_slug` in one batch.
/// Shared git plumbing between `rename_feature` and `rename_issue`;
/// only the ownership check differs between the two kinds,
/// so it is supplied by the caller as `reject_foreign`:
/// given the parsed file, return `Some(err)` when the memory does not belong to the caller's tracker kind,
/// mirroring `delete_feature`/`delete_issue`'s not-a-* guard,
/// or `None` to accept it into the rename.
///
/// Errors on a hard git failure, on an empty source directory, or on the first rejected file:
/// the caller's `E` must be constructible from [`ImportError`],
/// (`#[from]` on the wrapping variant already gives every tracker error type this for free).
pub(crate) async fn plan_slug_rename<E>(
    backend: &NativeBackend,
    entry: &GroupEntry,
    old_slug: &str,
    new_slug: &str,
    reject_foreign: impl Fn(&MemoryFile) -> Option<E>,
) -> Result<PlannedRename, E>
where
    E: From<ImportError>,
{
    let old_dir = format!("{MEMORIES_DIR}/{old_slug}");
    let entries = backend
        .list_tree(&entry.handle, &old_dir, &Rev::head())
        .await
        .map_err(|e| E::from(ImportError::Git(e)))?;
    if entries.is_empty() {
        return Err(E::from(ImportError::MemoryNotFound {
            slug: Some(old_slug.to_string()),
            id: None,
        }));
    }

    let mut moves: PlannedRename = Vec::with_capacity(entries.len() * 2);
    let mut moved = 0usize;
    for filename in &entries {
        let Some(stem) = filename.strip_suffix(MEMORY_EXTENSION) else {
            continue;
        };
        let Ok(id) = Uuid::parse_str(stem) else {
            continue;
        };
        let old_path = memory_path(old_slug, MemoryId::from_uuid(id));
        let new_path = memory_path(new_slug, MemoryId::from_uuid(id));
        let bytes = backend
            .read_file(&entry.handle, &old_path, &Rev::head())
            .await
            .map_err(|e| E::from(ImportError::Git(e)))?;

        let text = String::from_utf8_lossy(&bytes).into_owned();
        let file = MemoryFile::parse(&text).map_err(|e| E::from(ImportError::Parse(e)))?;
        if let Some(err) = reject_foreign(&file) {
            return Err(err);
        }

        moves.push((new_path, Some(bytes.to_vec())));
        moves.push((old_path, None));
        moved += 1;
    }
    if moved == 0 {
        return Err(E::from(ImportError::MemoryNotFound {
            slug: Some(old_slug.to_string()),
            id: None,
        }));
    }
    Ok(moves)
}

/// Count how many memory files under `slug`'s two-level directory are valid UUID-named entries.
/// Shared between `list_features_for_slug` and `list_issues_for_slug`; the walk and filter shape is identical.
/// The per-record read stays kind-specific (`read_feature` / `read_issue`) in each caller.
pub(crate) async fn count_slug_entries(
    backend: &NativeBackend,
    entry: &GroupEntry,
    slug: &str,
) -> Result<usize, ImportError> {
    let dir = format!("{MEMORIES_DIR}/{slug}");
    let filenames = backend
        .list_tree(&entry.handle, &dir, &Rev::head())
        .await
        .map_err(ImportError::Git)?;
    Ok(filenames
        .iter()
        .filter(|filename| {
            filename
                .strip_suffix(MEMORY_EXTENSION)
                .is_some_and(|stem| Uuid::parse_str(stem).is_ok())
        })
        .count())
}

/// A slug's single resolvable `(id, path)`, or the pre-decided [`ImportError`] a
/// `validate_memory_slug` + `resolve_by_slug` pair over the same `list_tree` result would
/// already have raised: an invalid slug name, zero UUID-named files (not-found), or two or
/// more (ambiguous). Named alias for [`read_all_slug_files`]'s intermediate candidate list.
type SlugCandidate = Result<(Uuid, String), ImportError>;

/// Per-slug outcome of a batched tracker listing read: either the slug's single memory file
/// parsed successfully, or the same [`ImportError`] a per-slug `validate_memory_slug` +
/// `resolve_by_slug` + `read_file` chain would have raised.
///
/// Shared batched I/O between `list_issues` and `list_features`: each caller still applies its
/// own kind-specific block-presence gating (`NotAFeature` / `NotAnIssue`) and `Finding` emission
/// against the parsed [`MemoryFile`]; only the git reads are batched here.
///
/// Reduces a listing over `N` slugs from `2N` sequential `spawn_blocking` git round trips (one
/// `list_tree` and one `read_file` per slug, each re-resolving the commit and root tree) to one
/// [`list_memory_slug_dirs`] walk plus one batched [`NativeBackend::read_files`] call, which
/// resolves the commit and root tree once and reuses them for every path.
pub(crate) async fn read_all_slug_files(
    backend: &NativeBackend,
    entry: &GroupEntry,
    rev: Rev,
) -> Result<Vec<(String, Result<MemoryFile, ImportError>)>, ImportError> {
    let slug_dirs = list_memory_slug_dirs(backend, &entry.handle, &rev)
        .await
        .map_err(ImportError::Git)?;

    let mut candidates: Vec<(String, SlugCandidate)> = Vec::with_capacity(slug_dirs.len());
    let mut paths: Vec<String> = Vec::new();
    for slug_dir in &slug_dirs {
        let candidate = validate_memory_slug(&slug_dir.slug).and_then(|()| {
            let uuids: Vec<Uuid> = slug_dir
                .filenames
                .iter()
                .filter_map(|name| {
                    name.strip_suffix(MEMORY_EXTENSION)
                        .and_then(|stem| Uuid::parse_str(stem).ok())
                })
                .collect();
            match uuids.len() {
                1 => Ok((
                    uuids[0],
                    memory_path(&slug_dir.slug, MemoryId::from_uuid(uuids[0])),
                )),
                0 => Err(ImportError::MemoryNotFound {
                    slug: Some(slug_dir.slug.clone()),
                    id: None,
                }),
                _ => Err(ImportError::MemoryAmbiguous {
                    slug: slug_dir.slug.clone(),
                    candidates: uuids,
                }),
            }
        });
        if let Ok((_, path)) = &candidate {
            paths.push(path.clone());
        }
        candidates.push((slug_dir.slug.clone(), candidate));
    }

    // One resolve of the commit and root tree, reused for every path below,
    // instead of once per slug like a `read_file`-per-slug loop would pay.
    let batch = backend
        .read_files(&entry.handle, paths, &rev)
        .await
        .map_err(ImportError::Git)?;
    let mut bytes_by_path = batch
        .into_iter()
        .collect::<std::collections::HashMap<_, _>>();

    let mut out = Vec::with_capacity(candidates.len());
    for (slug, candidate) in candidates {
        let outcome = match candidate {
            Err(err) => Err(err),
            Ok((id, path)) => match bytes_by_path.remove(&path) {
                Some(Ok(bytes)) => {
                    let text = String::from_utf8_lossy(&bytes).into_owned();
                    MemoryFile::parse(&text).map_err(ImportError::Parse)
                }
                Some(Err(GitError::PathNotFound(_))) => Err(ImportError::MemoryNotFound {
                    slug: Some(slug.clone()),
                    id: Some(id),
                }),
                Some(Err(err)) => Err(ImportError::Git(err)),
                // `read_files` returns exactly one outcome per requested path; a path built
                // from this same loop missing from its own result is a broken batching
                // invariant, not a reachable runtime state.
                None => unreachable!("read_files omitted a requested path"),
            },
        };
        out.push((slug, outcome));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use crate::memory::import_memory;
    use crate::testing::ScratchHome;
    use mmcp_core::memory::{
        FeatureMetadata, FeatureStatus, FrontmatterFormat, MemoryFrontmatter, MemoryKind,
    };

    /// Regression guard for the `checked_add` fix: a frontmatter `feature.number` sitting at
    /// `u32::MAX` (externally writable, no upper bound enforced at write time) must surface a
    /// typed [`ImportError::TicketCounterOverflow`] instead of wrapping to `0` (release) or
    /// panicking (debug) and reissuing an already-allocated ticket number.
    #[tokio::test]
    async fn next_ticket_number_errors_on_counter_overflow() {
        let scratch = ScratchHome::new().await.expect("scratch home");
        let seeded = scratch.seed_group("overflow-group").await.expect("seed");
        let entry = scratch.groups().get(&seeded.group_id).await.expect("entry");

        let file = MemoryFile {
            frontmatter: MemoryFrontmatter::new("Maxed out", "at the ceiling", MemoryKind::Feature)
                .with_feature(FeatureMetadata {
                    status: FeatureStatus::Requested,
                    number: Some(u32::MAX),
                    ..FeatureMetadata::default()
                }),
            body: "## Need\n\nAt the ceiling.\n".to_string(),
            format: FrontmatterFormat::TomlPlus,
        };
        let rendered = file.to_string().expect("render maxed-out feature");
        import_memory(
            scratch.backend(),
            &entry.handle,
            "maxed-out",
            &rendered,
            None,
            scratch.author(),
            false,
        )
        .await
        .expect("seed maxed-out feature");

        let err = next_ticket_number(scratch.backend(), &entry)
            .await
            .expect_err("counter at u32::MAX must error instead of wrapping");
        assert!(matches!(err, ImportError::TicketCounterOverflow));
    }

    /// Below the ceiling, the counter still mints the next number normally.
    #[tokio::test]
    async fn next_ticket_number_increments_normally_below_ceiling() {
        let scratch = ScratchHome::new().await.expect("scratch home");
        let seeded = scratch.seed_group("normal-group").await.expect("seed");
        let entry = scratch.groups().get(&seeded.group_id).await.expect("entry");

        let file = MemoryFile {
            frontmatter: MemoryFrontmatter::new("Feature 5", "regular ticket", MemoryKind::Feature)
                .with_feature(FeatureMetadata {
                    status: FeatureStatus::Requested,
                    number: Some(5),
                    ..FeatureMetadata::default()
                }),
            body: "## Need\n\nRegular ticket.\n".to_string(),
            format: FrontmatterFormat::TomlPlus,
        };
        let rendered = file.to_string().expect("render feature 5");
        import_memory(
            scratch.backend(),
            &entry.handle,
            "feature-5",
            &rendered,
            None,
            scratch.author(),
            false,
        )
        .await
        .expect("seed feature 5");

        let next = next_ticket_number(scratch.backend(), &entry)
            .await
            .expect("next ticket number");
        assert_eq!(next, 6);
    }

    /// Two files under one slug directory (an ambiguous slug, reachable via direct git surgery
    /// or an externally imported repo) both contribute their `feature.number` to the counter.
    /// The higher number sits in the lexicographically LARGER filename: git tree listings are
    /// name-sorted, so a wrong fix that reads only the first listed file per slug (the
    /// [`read_all_slug_files`] collapse this fix must not reuse) would cap the counter at the
    /// lower number and fail this assertion, instead of passing by luck on a random UUID order.
    #[tokio::test]
    async fn next_ticket_number_folds_every_file_of_an_ambiguous_slug() {
        let scratch = ScratchHome::new().await.expect("scratch home");
        let seeded = scratch.seed_group("ambiguous-group").await.expect("seed");
        let entry = scratch.groups().get(&seeded.group_id).await.expect("entry");

        let baseline = MemoryFile {
            frontmatter: MemoryFrontmatter::new("Low ticket", "baseline", MemoryKind::Feature)
                .with_feature(FeatureMetadata {
                    status: FeatureStatus::Requested,
                    number: Some(3),
                    ..FeatureMetadata::default()
                }),
            body: "## Need\n\nBaseline.\n".to_string(),
            format: FrontmatterFormat::TomlPlus,
        };
        import_memory(
            scratch.backend(),
            &entry.handle,
            "low-ticket",
            &baseline.to_string().expect("render baseline feature"),
            None,
            scratch.author(),
            false,
        )
        .await
        .expect("seed baseline feature");

        let smaller_filename_id = MemoryId::from_uuid(
            Uuid::parse_str("00000000-0000-0000-0000-000000000001").expect("valid uuid"),
        );
        let larger_filename_id = MemoryId::from_uuid(
            Uuid::parse_str("ffffffff-ffff-ffff-ffff-ffffffffffff").expect("valid uuid"),
        );
        let low_number_file = MemoryFile {
            frontmatter: MemoryFrontmatter::new("Ambiguous low", "d", MemoryKind::Feature)
                .with_feature(FeatureMetadata {
                    status: FeatureStatus::Requested,
                    number: Some(20),
                    ..FeatureMetadata::default()
                }),
            body: "b".to_string(),
            format: FrontmatterFormat::TomlPlus,
        };
        let high_number_file = MemoryFile {
            frontmatter: MemoryFrontmatter::new("Ambiguous high", "d", MemoryKind::Feature)
                .with_feature(FeatureMetadata {
                    status: FeatureStatus::Requested,
                    number: Some(99),
                    ..FeatureMetadata::default()
                }),
            body: "b".to_string(),
            format: FrontmatterFormat::TomlPlus,
        };
        let author = scratch.author();
        scratch
            .backend()
            .write_commit(
                &entry.handle,
                mmcp_git::CommitSpec::mmcp_commit(
                    "seed ambiguous slug".to_string(),
                    vec![
                        (
                            memory_path("ambiguous-feature", smaller_filename_id),
                            Some(
                                low_number_file
                                    .to_string()
                                    .expect("render low-number file")
                                    .into_bytes(),
                            ),
                        ),
                        (
                            memory_path("ambiguous-feature", larger_filename_id),
                            Some(
                                high_number_file
                                    .to_string()
                                    .expect("render high-number file")
                                    .into_bytes(),
                            ),
                        ),
                    ],
                    &author.name,
                    &author.email,
                ),
            )
            .await
            .expect("seed ambiguous slug files");

        let next = next_ticket_number(scratch.backend(), &entry)
            .await
            .expect("an ambiguous slug must not abort the counter");
        assert_eq!(next, 100, "must fold both files, not just the first listed");
    }

    /// Path count the batch-count test below seeds, large enough that a per-path
    /// `read_file` loop and a single `read_files` batch call are trivially distinguishable.
    const BULK_PATH_COUNT: usize = 25;

    /// [`max_ticket_number_over`] calls its `read_batch` seam exactly once, independent of how
    /// many paths it folds. The regression this guards is a per-path `read_file` loop, which
    /// would call the seam once per path instead of once for the whole group.
    #[tokio::test]
    async fn max_ticket_number_over_reads_the_batch_exactly_once() {
        let paths: Vec<String> = (0..BULK_PATH_COUNT)
            .map(|i| format!("memories/bulk-{i}/dummy.md"))
            .collect();
        let requested_len = paths.len();
        let call_count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counted = std::sync::Arc::clone(&call_count);

        let max = max_ticket_number_over(paths, move |batched_paths| {
            counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            assert_eq!(
                batched_paths.len(),
                requested_len,
                "every path must land in the single batch call"
            );
            async { Ok(BatchOutcome::new()) }
        })
        .await
        .expect("an empty batch still resolves to max = 0");

        assert_eq!(
            call_count.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "the read seam must be called exactly once regardless of path count"
        );
        assert_eq!(max, 0);
    }
}
