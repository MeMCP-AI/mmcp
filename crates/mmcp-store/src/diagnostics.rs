//! Health (surface) and diagnostic (deep) checks for memory repos.
//!
//! - **Health**: quick pass/fail per group, manifest validity and frontmatter parse,
//!   returns counts and errors only.
//! - **Diagnose**: deep analysis: missing optional fields, naming drift, empty groups,
//!   semver parse issues, directory/manifest UUID mismatches, and structural hints.
//!
//! Neither endpoint ever modifies data.
//! Consumers, CLI check/diagnose, MCP `check_health`/`diagnose` tools, GUI diagnostic widgets,
//! receive typed `GroupReport`/`DiagReport` structs and decide how to render them.
//!
//! CLI runners (`run_check`, `run_diagnose`, `print_reports`) live in
//! the client crate: they carry exit-code and stdout shaping that
//! belongs in the binary.
//!
//! Naming: `Finding` is the per-check record, avoiding a lexical collision with `MemoryKind::Issue`.

use mmcp_core::manifest::{GroupScope, MANIFEST_SCHEMA_VERSION};
use mmcp_core::memory::{
    MemoryFile, MemoryFrontmatter, MemoryKind, parse_frontmatter, parse_sections,
};
use mmcp_git::{GitBackend, GitError, NativeBackend, RepoHandle, Rev};
use serde::Serialize;
use uuid::Uuid;

use crate::groups::{GroupEntry, GroupIndex};
use crate::home::{MmcpHome, read_git_global};
use crate::memory::{MemoryFileRef, slugify_filename};

// ── Shared types ────────────────────────────────────────────

/// One finding emitted by a check.
/// `code` is a stable slug-style identifier (e.g. `manifest_unreadable`, `memory_body_empty`).
/// It lets consumers branch without parsing the free-form `message`.
/// The MCP tool boundary maps each finding onto a `mmcp_proto::Note` using this code.
#[derive(Debug, Clone, Serialize)]
pub struct Finding {
    pub group: String,
    pub slug: Option<String>,
    pub severity: &'static str,
    pub code: &'static str,
    pub message: String,
}

/// Report for a single group.
#[derive(Debug, Clone, Serialize)]
pub struct GroupReport {
    pub group_id: String,
    pub slug: String,
    pub manifest_ok: bool,
    pub memory_count: usize,
    pub findings: Vec<Finding>,
}

/// Full diagnostic report including project-level findings.
#[derive(Debug, Clone, Serialize)]
pub struct DiagReport {
    pub project_findings: Vec<Finding>,
    pub groups: Vec<GroupReport>,
}

// ── Health (surface) ────────────────────────────────────────

/// Quick surface check: manifest parseable, all memories parse, required fields present.
/// No hints, no deep analysis.
pub async fn health_check_group(backend: &NativeBackend, entry: &GroupEntry) -> GroupReport {
    let gid = entry.manifest.group_id.as_uuid().to_string();
    let slug = entry.manifest.slug.clone();
    let mut findings = Vec::new();

    // Manifest
    let manifest_ok = match backend.read_manifest(&entry.handle).await {
        Ok(m) => {
            if m.schema_version > MANIFEST_SCHEMA_VERSION {
                findings.push(Finding {
                    group: gid.clone(),
                    slug: None,
                    severity: "error",
                    code: "manifest_schema_too_new",
                    message: format!(
                        "manifest schema_version {} is newer than supported {}",
                        m.schema_version, MANIFEST_SCHEMA_VERSION
                    ),
                });
            }
            if m.slug.is_empty() {
                findings.push(Finding {
                    group: gid.clone(),
                    slug: None,
                    severity: "error",
                    code: "manifest_slug_empty",
                    message: "manifest slug is empty".to_string(),
                });
            }
            true
        }
        Err(err) => {
            findings.push(Finding {
                group: gid.clone(),
                slug: None,
                severity: "error",
                code: "manifest_unreadable",
                message: format!("manifest unreadable: {err}"),
            });
            false
        }
    };

    // Memories
    let rev = Rev::head();
    let files = match crate::memory::list_all_memory_files(backend, &entry.handle, &rev).await {
        Ok(f) => f,
        Err(err) => {
            findings.push(Finding {
                group: gid.clone(),
                slug: None,
                severity: "warning",
                code: "memories_list_failed",
                message: format!("cannot list memories/: {err}"),
            });
            return GroupReport {
                group_id: gid,
                slug,
                manifest_ok,
                memory_count: 0,
                findings,
            };
        }
    };

    let mut memory_count = 0;
    for file in &files {
        let mem_slug = file.slug.as_str();
        memory_count += 1;

        match backend.read_file(&entry.handle, &file.path, &rev).await {
            Ok(bytes) => {
                // Shared non-UTF8 finding constructor: see `crate::tracker::not_utf8_finding`.
                // Preserves the underlying `Utf8Error` in the message.
                let text = match std::str::from_utf8(&bytes) {
                    Ok(text) => text,
                    Err(err) => {
                        findings.push(crate::tracker::not_utf8_finding(&gid, mem_slug, &err));
                        continue;
                    }
                };
                if let Err(err) = MemoryFile::parse(text) {
                    findings.push(crate::tracker::parse_failed_finding(&gid, mem_slug, &err));
                }
            }
            Err(err) => {
                findings.push(Finding {
                    group: gid.clone(),
                    slug: Some(mem_slug.to_string()),
                    severity: "error",
                    code: "memory_read_failed",
                    message: format!("cannot read: {err}"),
                });
            }
        }
    }

    GroupReport {
        group_id: gid,
        slug,
        manifest_ok,
        memory_count,
        findings,
    }
}

pub async fn health_check_all(backend: &NativeBackend, groups: &GroupIndex) -> Vec<GroupReport> {
    let entries = groups.list().await;
    let mut reports = Vec::with_capacity(entries.len());
    for entry in entries {
        reports.push(health_check_group(backend, &entry).await);
    }
    reports
}

// ── Diagnose (deep) ─────────────────────────────────────────

/// Deep diagnostic analysis.
/// Runs everything health does plus:
/// - Missing optional fields (tags, version) as info hints
/// - Empty body detection
/// - Empty group (zero memories)
/// - Manifest group_id vs directory UUID mismatch
/// - Manifest created_at sanity
/// - Slug/name drift (filename slug != frontmatter name)
/// - Semver parse issues in version field
/// - Duplicate slugs across groups
pub async fn diagnose_group(backend: &NativeBackend, entry: &GroupEntry) -> GroupReport {
    let mut report = health_check_group(backend, entry).await;
    let gid = report.group_id.clone();
    let rev = Rev::head();
    let group_scope = entry.manifest.scope;

    // Tracks feature numbers seen within this group to flag duplicates after the per-memory loop.
    // `add_feature` enforces monotonic uniqueness on writes, but a manual edit could reintroduce a collision.
    let mut feature_numbers: std::collections::HashMap<u32, Vec<String>> =
        std::collections::HashMap::new();

    // Indexes every feature in this group by its UUID so the post-loop supersede-chain integrity pass
    // can look targets up without a second pass over the files.
    // Value is `(slug, status, refs)`, enough to verify reciprocity without carrying the full frontmatter around.
    let mut features_by_id: std::collections::HashMap<
        Uuid,
        (
            String,
            mmcp_core::memory::FeatureStatus,
            Vec<mmcp_core::memory::MemoryRef>,
        ),
    > = std::collections::HashMap::new();

    // Collected `(slug, superseded_by)` pairs so the post-loop
    // pass can walk them in deterministic order.
    let mut supersede_links: Vec<(String, Uuid, mmcp_core::memory::MemoryRef)> = Vec::new();

    // Deep manifest checks
    if let Ok(m) = backend.read_manifest(&entry.handle).await {
        // group_id vs directory UUID
        let dir_uuid = entry.handle.group_id.to_string();
        let manifest_uuid = m.group_id.as_uuid().to_string();
        if dir_uuid != manifest_uuid {
            report.findings.push(Finding {
                group: gid.clone(),
                slug: None,
                severity: "error",
                code: "manifest_group_id_mismatch",
                message: format!(
                    "manifest group_id {manifest_uuid} does not match directory UUID {dir_uuid}"
                ),
            });
        }

        // created_at sanity
        if m.created_at <= 0 {
            report.findings.push(Finding {
                group: gid.clone(),
                slug: None,
                severity: "warning",
                code: "manifest_created_at_invalid",
                message: format!(
                    "manifest created_at is {}, expected positive timestamp",
                    m.created_at
                ),
            });
        }

        // display_name hint
        if m.display_name.is_none() {
            report.findings.push(Finding {
                group: gid.clone(),
                slug: None,
                severity: "info",
                code: "manifest_no_display_name",
                message: "manifest has no display_name set".to_string(),
            });
        }
    }

    // Empty group
    if report.memory_count == 0 {
        report.findings.push(Finding {
            group: gid.clone(),
            slug: None,
            severity: "info",
            code: "group_empty",
            message: "group has zero memories".to_string(),
        });
    }

    // Deep per-memory checks
    let files = crate::memory::list_all_memory_files(backend, &entry.handle, &rev)
        .await
        .unwrap_or_default();

    for file_ref in &files {
        let mem_slug = file_ref.slug.as_str();
        let Ok(bytes) = backend.read_file(&entry.handle, &file_ref.path, &rev).await else {
            continue;
        };
        let Ok(text) = std::str::from_utf8(&bytes) else {
            continue;
        };
        let Ok(file) = MemoryFile::parse(text) else {
            continue; // Already reported by health
        };
        let fm = &file.frontmatter;

        // Required field quality
        if fm.name.is_empty() {
            report.findings.push(Finding {
                group: gid.clone(),
                slug: Some(mem_slug.to_string()),
                severity: "error",
                code: "memory_name_empty",
                message: "name is empty".to_string(),
            });
        }
        if fm.description.is_empty() {
            report.findings.push(Finding {
                group: gid.clone(),
                slug: Some(mem_slug.to_string()),
                severity: "error",
                code: "memory_description_empty",
                message: "description is empty".to_string(),
            });
        }

        // Body quality
        if file.body.trim().is_empty() {
            report.findings.push(Finding {
                group: gid.clone(),
                slug: Some(mem_slug.to_string()),
                severity: "warning",
                code: "memory_body_empty",
                message: "body is empty".to_string(),
            });
        }

        // Tags hint
        if fm.tags.is_empty() {
            report.findings.push(Finding {
                group: gid.clone(),
                slug: Some(mem_slug.to_string()),
                severity: "info",
                code: "memory_no_tags",
                message: "no tags set (reduces discoverability)".to_string(),
            });
        }

        // Version field parse check
        if let Some(ref v) = fm.version {
            // Already parsed as semver by serde, but check it's reasonable
            if v.major == 0 && v.minor == 0 && v.patch == 0 {
                report.findings.push(Finding {
                    group: gid.clone(),
                    slug: Some(mem_slug.to_string()),
                    severity: "info",
                    code: "memory_version_zero",
                    message: "version is 0.0.0 (not yet published?)".to_string(),
                });
            }
        }

        // Slug/name drift fires only when the slug and slugified name share *no* meaningful tokens.
        // Substring containment is too loose,
        // since a short curated slug like "global-coding-rules-rust" and a full title like "Rust Coding Rules"
        // legitimately differ as abbreviation vs. expansion.
        // Zero-overlap is a strong signal the name actually changed without the slug rotating.
        let name_slug = slugify_filename(&fm.name);
        if !name_slug.is_empty() && name_slug != mem_slug && !tokens_overlap(mem_slug, &name_slug) {
            report.findings.push(Finding {
                group: gid.clone(),
                slug: Some(mem_slug.to_string()),
                severity: "info",
                code: "slug_name_drift",
                message: format!(
                    "slug '{mem_slug}' and name '{}' share no common tokens (slugified name: '{name_slug}') — rename may have drifted",
                    fm.name
                ),
            });
        }

        // Frontmatter `id` must match the filename UUID.
        match fm.id {
            None => report.findings.push(Finding {
                group: gid.clone(),
                slug: Some(mem_slug.to_string()),
                severity: "error",
                code: "memory_missing_id",
                message: format!(
                    "frontmatter has no `id`; expected {} (FR-028 requires every memory to carry its UUID)",
                    file_ref.id
                ),
            }),
            Some(fid) if fid != file_ref.id => report.findings.push(Finding {
                group: gid.clone(),
                slug: Some(mem_slug.to_string()),
                severity: "error",
                code: "memory_id_filename_mismatch",
                message: format!(
                    "frontmatter id {fid} does not match filename id {} at {}",
                    file_ref.id, file_ref.path
                ),
            }),
            _ => {}
        }

        // Every `kind = "feature"` memory should carry a sequential `number`.
        if fm.kind == MemoryKind::Feature && fm.feature.as_ref().and_then(|f| f.number).is_none() {
            report.findings.push(Finding {
                group: gid.clone(),
                slug: Some(mem_slug.to_string()),
                severity: "info",
                code: "feature_no_number",
                message: "feature has no `number` metadata (FR-027 auto-assigns on create)"
                    .to_string(),
            });
        }

        // Kind vs `[feature]` subtable consistency.
        // A `feature` kind must carry a `[feature]` block; no other kind may.
        match (fm.kind, fm.feature.as_ref()) {
            (MemoryKind::Feature, None) => report.findings.push(Finding {
                group: gid.clone(),
                slug: Some(mem_slug.to_string()),
                severity: "error",
                code: "feature_no_subtable",
                message: "kind = \"feature\" but frontmatter has no `[feature]` subtable"
                    .to_string(),
            }),
            (other, Some(_)) if other != MemoryKind::Feature => {
                report.findings.push(Finding {
                    group: gid.clone(),
                    slug: Some(mem_slug.to_string()),
                    severity: "error",
                    code: "non_feature_has_subtable",
                    message: format!(
                        "kind = \"{}\" carries a stray `[feature]` subtable; only `feature` should",
                        other.as_str()
                    ),
                });
            }
            _ => {}
        }

        // Feature self-reference: a feature listing its own id in
        // `depends_on` or `blocks` is almost certainly a copy-paste
        // slip.
        if let Some(feat) = fm.feature.as_ref() {
            let self_id = file_ref.id;
            for (field, refs) in [("depends_on", &feat.depends_on), ("blocks", &feat.blocks)] {
                if refs.contains(&self_id) {
                    report.findings.push(Finding {
                        group: gid.clone(),
                        slug: Some(mem_slug.to_string()),
                        severity: "info",
                        code: "feature_self_reference",
                        message: format!(
                            "feature `{field}` references its own id {self_id} (self-reference)"
                        ),
                    });
                }
            }
            if let Some(n) = feat.number {
                feature_numbers
                    .entry(n)
                    .or_default()
                    .push(mem_slug.to_string());
            }
            if let Some(link) = feat.superseded_by.as_ref() {
                supersede_links.push((mem_slug.to_string(), file_ref.id, link.clone()));
            }
            features_by_id.insert(
                file_ref.id,
                (mem_slug.to_string(), feat.status, fm.refs.clone()),
            );
        }

        // A `mandatory = true` memory living in a non-`global` group
        // only fans out to projects that explicitly adopt the group,
        // rarely what authors intend for mandatory-read rules.
        if fm.mandatory && group_scope != GroupScope::Global {
            report.findings.push(Finding {
                group: gid.clone(),
                slug: Some(mem_slug.to_string()),
                severity: "info",
                code: "mandatory_in_non_global_scope",
                message: format!(
                    "mandatory memory in `scope = \"{}\"` group — FR-025 only fans it out to matching projects; set group scope to `global` for cross-project propagation",
                    scope_str(group_scope)
                ),
            });
        }

        // Surface malformed CommonMark sections so a body that
        // `edit_memory_body` would choke on is visible in
        // diagnostics.
        if let Err(err) = parse_sections(&file.body) {
            report.findings.push(Finding {
                group: gid.clone(),
                slug: Some(mem_slug.to_string()),
                severity: "warning",
                code: "memory_body_sections_invalid",
                message: format!("body failed FR-026 section parse: {err}"),
            });
        }
    }

    // Supersede-chain integrity: every feature with a typed `superseded_by` back-link
    // should have its target present in the same group,
    // reciprocating via a `refs` entry pointing back at the source's UUID.
    // Catches the half-landed two-commit supersede case: commit A wrote the new feature,
    // commit B never flipped the old one's status, or the reverse,
    // commit B flipped it but an operator later removed the new feature's ref by hand.
    for (old_slug, old_uuid, link) in &supersede_links {
        match features_by_id.get(&link.target) {
            None => {
                report.findings.push(Finding {
                    group: gid.clone(),
                    slug: Some(old_slug.clone()),
                    severity: "warning",
                    code: "supersede_target_missing",
                    message: format!(
                        "supersede target {} is not present in this group — cross-group supersede is not supported in v1",
                        link.target
                    ),
                });
            }
            Some((new_slug, _status, new_refs)) => {
                if !new_refs.iter().any(|r| r.target == *old_uuid) {
                    report.findings.push(Finding {
                        group: gid.clone(),
                        slug: Some(old_slug.clone()),
                        severity: "warning",
                        code: "supersede_one_sided",
                        message: format!(
                            "supersede chain one-sided: `{old_slug}` points at `{new_slug}` via superseded_by, but `{new_slug}`'s `refs` does not reference `{old_slug}` back"
                        ),
                    });
                }
            }
        }
    }

    // Duplicate feature numbers within this group.
    for (number, slugs) in feature_numbers {
        if slugs.len() > 1 {
            for slug in &slugs {
                report.findings.push(Finding {
                    group: gid.clone(),
                    slug: Some(slug.clone()),
                    severity: "warning",
                    code: "feature_number_duplicate",
                    message: format!(
                        "feature number {number} is shared with {} other feature(s): {}",
                        slugs.len() - 1,
                        slugs
                            .iter()
                            .filter(|s| *s != slug)
                            .cloned()
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                });
            }
        }
    }

    // Walk every leaf slug dir so stray non-UUID filenames are surfaced.
    // Nested slug paths surface as separate leaf entries; intermediate path nodes are not "empty leaves".
    // `list_all_memory_files` silently skips non-UUID files,
    // so without this pass a `memories/rules/scratch.md` would never show up in diagnostics.
    if let Ok(slug_dirs) = crate::memory::list_memory_slug_dirs(backend, &entry.handle, &rev).await
    {
        for slug_dir in slug_dirs {
            let mut valid_memory_files = 0usize;
            for filename in &slug_dir.filenames {
                let Some(stem) = filename.strip_suffix(mmcp_core::conventions::MEMORY_EXTENSION)
                else {
                    report.findings.push(Finding {
                        group: gid.clone(),
                        slug: Some(slug_dir.slug.clone()),
                        severity: "warning",
                        code: "slug_dir_non_memory_file",
                        message: format!(
                            "non-memory file '{filename}' under {}/ (expected `<uuid>.md`)",
                            slug_dir.dir
                        ),
                    });
                    continue;
                };
                if Uuid::parse_str(stem).is_err() {
                    report.findings.push(Finding {
                        group: gid.clone(),
                        slug: Some(slug_dir.slug.clone()),
                        severity: "error",
                        code: "slug_dir_bad_filename",
                        message: format!(
                            "memory filename '{filename}' under {}/ is not a valid UUID",
                            slug_dir.dir
                        ),
                    });
                    continue;
                }
                valid_memory_files += 1;
            }
            if valid_memory_files == 0 {
                report.findings.push(Finding {
                    group: gid.clone(),
                    slug: Some(slug_dir.slug.clone()),
                    severity: "info",
                    code: "slug_dir_empty",
                    message: format!(
                        "slug directory {}/ has no memory files (leftover from a rename or delete?)",
                        slug_dir.dir
                    ),
                });
            }
        }
    }

    report
}

/// Human-readable name of a [`GroupScope`] for diagnostic messages.
/// The serde rename_all derives the same lowercase tokens quoted back at operators.
fn scope_str(scope: GroupScope) -> &'static str {
    match scope {
        GroupScope::Global => "global",
        GroupScope::Shared => "shared",
        GroupScope::Project => "project",
    }
}

// ── Shared frontmatter corpus for diagnose_all's cross-group passes ────

/// One memory file failed to read, decode as UTF-8, or parse while building
/// a [`GroupFrontmatterCorpus`] entry. Carries no payload: every downstream
/// pass skips a failed entry silently, matching the per-file
/// `let Ok(x) = ... else { continue }` guard each pass used before this
/// corpus existed, so the specific cause is never inspected.
#[derive(Debug)]
struct CorpusEntryError;

/// Every memory in one group, paired with its frontmatter-parse outcome.
/// Built by exactly one [`crate::memory::list_all_memory_files`] listing, one
/// batched [`NativeBackend::read_files`] read, and one [`parse_frontmatter`]
/// call per file. The slug-dup, by-id, cross-ref, and milestone-rollup
/// passes in [`diagnose_all`] each need only frontmatter fields (kind,
/// feature, milestone, id), never the body, so they share this corpus
/// instead of each re-walking and re-reading the same files with its own
/// full [`MemoryFile::parse`].
///
/// Materializes every memory's frontmatter for the group at once, the same
/// whole-corpus-in-RAM trade `mmcp_store::tracker::read_all_slug_files`
/// documents on its own doc comment.
type GroupFrontmatterCorpus = Vec<(MemoryFileRef, Result<MemoryFrontmatter, CorpusEntryError>)>;

/// Build a [`GroupFrontmatterCorpus`] from an already-listed set of memory
/// files, given a caller-supplied batch-read seam. The seam is called
/// exactly once, with every file's path in one request, regardless of how
/// many of `diagnose_all`'s four frontmatter-only passes end up consuming
/// the returned corpus afterward: the fault this corpus replaces was each
/// of those passes running its own `read_file`-per-path loop.
async fn build_frontmatter_corpus<F, Fut>(
    files: Vec<MemoryFileRef>,
    mut read_batch: F,
) -> GroupFrontmatterCorpus
where
    F: FnMut(Vec<String>) -> Fut,
    Fut: Future<Output = Result<Vec<(String, Result<bytes::Bytes, GitError>)>, GitError>>,
{
    let paths: Vec<String> = files.iter().map(|f| f.path.clone()).collect();
    let bytes_by_path: std::collections::HashMap<String, Result<bytes::Bytes, GitError>> =
        match read_batch(paths).await {
            Ok(batch) => batch.into_iter().collect(),
            // A batch-level failure (e.g. the revision does not resolve)
            // fails every file identically, matching the pre-refactor
            // per-file `read_file` loop, where the same failure would have
            // hit every individual call the same way.
            Err(_) => std::collections::HashMap::new(),
        };

    files
        .into_iter()
        .map(|file_ref| {
            let outcome = match bytes_by_path.get(&file_ref.path) {
                Some(Ok(bytes)) => match std::str::from_utf8(bytes) {
                    Ok(text) => parse_frontmatter(text).map_err(|_| CorpusEntryError),
                    Err(_) => Err(CorpusEntryError),
                },
                _ => Err(CorpusEntryError),
            };
            (file_ref, outcome)
        })
        .collect()
}

/// Build a [`GroupFrontmatterCorpus`] for one group at `rev`: one
/// [`crate::memory::list_all_memory_files`] listing feeding one
/// [`NativeBackend::read_files`] batch read via [`build_frontmatter_corpus`].
async fn read_group_frontmatter_corpus(
    backend: &NativeBackend,
    handle: &RepoHandle,
    rev: &Rev,
) -> Result<GroupFrontmatterCorpus, GitError> {
    let files = crate::memory::list_all_memory_files(backend, handle, rev).await?;
    Ok(build_frontmatter_corpus(files, |paths| backend.read_files(handle, paths, rev)).await)
}

pub async fn diagnose_all(backend: &NativeBackend, groups: &GroupIndex) -> DiagReport {
    let mut project_findings = Vec::new();

    // Project-level: check if a sync server is configured
    check_project_config(&mut project_findings);

    let entries = groups.list().await;
    let mut reports = Vec::with_capacity(entries.len());

    // Collect all slugs for cross-group duplicate detection
    let mut slug_groups: std::collections::HashMap<String, Vec<String>> =
        std::collections::HashMap::new();

    // One shared frontmatter corpus per group, indexed in lockstep with
    // `entries`, feeding the slug-dup, by-id, cross-ref, and
    // milestone-rollup passes below without any of them re-listing or
    // re-reading the group's memory files on their own.
    let mut corpora: Vec<GroupFrontmatterCorpus> = Vec::with_capacity(entries.len());

    for entry in &entries {
        let report = diagnose_group(backend, entry).await;
        let gid = report.group_id.clone();
        let rev = Rev::head();
        let corpus = read_group_frontmatter_corpus(backend, &entry.handle, &rev)
            .await
            .unwrap_or_default();

        // Dedupe per-group: under the two-level layout a slug
        // with multiple UUIDs is still one slug from the
        // cross-group-duplicate perspective.
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        for (file_ref, _) in &corpus {
            if seen.insert(file_ref.slug.clone()) {
                slug_groups
                    .entry(file_ref.slug.clone())
                    .or_default()
                    .push(gid.clone());
            }
        }

        corpora.push(corpus);
        reports.push(report);
    }

    // Cross-group slug duplicates
    for (dup_slug, group_ids) in &slug_groups {
        if group_ids.len() > 1 {
            for report in &mut reports {
                if group_ids.contains(&report.group_id) {
                    report.findings.push(Finding {
                        group: report.group_id.clone(),
                        slug: Some(dup_slug.clone()),
                        severity: "info",
                        code: "slug_cross_group_duplicate",
                        message: format!(
                            "slug '{dup_slug}' also exists in {} other group(s)",
                            group_ids.len() - 1
                        ),
                    });
                }
            }
        }
    }

    // Build a single registry of every memory across the mirror so downstream cross-ref validation
    // and duplicate-UUID detection share one pass over the repos.
    // Each entry is keyed by UUID; the value carries enough context
    // to point operators at the offending file when a collision fires.
    #[derive(Clone)]
    struct MemoryRecord {
        group_id: String,
        slug: String,
        kind: MemoryKind,
    }
    let mut by_id: std::collections::HashMap<Uuid, Vec<MemoryRecord>> =
        std::collections::HashMap::new();
    for (entry, corpus) in entries.iter().zip(&corpora) {
        let gid = entry.handle.group_id.to_string();
        for (file_ref, outcome) in corpus {
            let Ok(fm) = outcome else {
                continue;
            };
            by_id.entry(file_ref.id).or_default().push(MemoryRecord {
                group_id: gid.clone(),
                slug: file_ref.slug.clone(),
                kind: fm.kind,
            });
        }
    }

    // Duplicate UUIDs across memories.
    // Every memory's UUID is its primary key; two memories sharing one breaks `resolve_by_id`
    // and makes cross-refs ambiguous.
    for (uuid, records) in &by_id {
        if records.len() > 1 {
            let locations: Vec<String> = records
                .iter()
                .map(|r| format!("{}/{}", r.group_id, r.slug))
                .collect();
            for record in records {
                if let Some(report) = reports.iter_mut().find(|r| r.group_id == record.group_id) {
                    report.findings.push(Finding {
                        group: record.group_id.clone(),
                        slug: Some(record.slug.clone()),
                        severity: "error",
                        code: "memory_id_duplicate",
                        message: format!(
                            "duplicate memory id {uuid} also exists at {}",
                            locations
                                .iter()
                                .filter(
                                    |loc| *loc != &format!("{}/{}", record.group_id, record.slug)
                                )
                                .cloned()
                                .collect::<Vec<_>>()
                                .join(", ")
                        ),
                    });
                }
            }
        }
    }

    // Cross-ref validation for features: `depends_on` / `blocks`
    // must resolve to an existing memory, and that memory must
    // itself be a feature.
    for (entry, corpus) in entries.iter().zip(&corpora) {
        let gid = entry.handle.group_id.to_string();
        for (file_ref, outcome) in corpus {
            let Ok(fm) = outcome else {
                continue;
            };
            if fm.kind != MemoryKind::Feature {
                continue;
            }
            let Some(feat) = fm.feature.as_ref() else {
                continue;
            };
            let Some(report) = reports.iter_mut().find(|r| r.group_id == gid) else {
                continue;
            };
            for (field, refs) in [("depends_on", &feat.depends_on), ("blocks", &feat.blocks)] {
                for uuid in refs {
                    match by_id.get(uuid) {
                        None => report.findings.push(Finding {
                            group: gid.clone(),
                            slug: Some(file_ref.slug.clone()),
                            severity: "warning",
                            code: "feature_ref_dangling",
                            message: format!(
                                "feature `{field}` references unknown memory {uuid} — dangling cross-reference"
                            ),
                        }),
                        Some(records) => {
                            // Pick the first record; duplicates
                            // get flagged separately above.
                            if let Some(record) = records.first()
                                && record.kind != MemoryKind::Feature
                            {
                                report.findings.push(Finding {
                                    group: gid.clone(),
                                    slug: Some(file_ref.slug.clone()),
                                    severity: "warning",
                                    code: "feature_ref_wrong_kind",
                                    message: format!(
                                        "feature `{field}` points at {uuid} which is kind = \"{}\", not `feature`",
                                        record.kind.as_str()
                                    ),
                                });
                            }
                        }
                    }
                }
            }

            // Milestone reference: `feature.milestone` must resolve
            // to an existing memory, and that memory must itself be
            // a milestone, regardless of which group it lives in.
            // `by_id` (already built cross-group above) is exactly
            // the right registry to check against, no extra git
            // walk needed.
            if let Some(milestone_id) = feat.milestone {
                match by_id.get(&milestone_id) {
                    None => report.findings.push(Finding {
                        group: gid.clone(),
                        slug: Some(file_ref.slug.clone()),
                        severity: "warning",
                        code: "milestone_ref_dangling",
                        message: format!(
                            "feature `milestone` references unknown memory {milestone_id} — orphaned milestone reference"
                        ),
                    }),
                    Some(records) => {
                        if let Some(record) = records.first()
                            && record.kind != MemoryKind::Milestone
                        {
                            report.findings.push(Finding {
                                group: gid.clone(),
                                slug: Some(file_ref.slug.clone()),
                                severity: "warning",
                                code: "milestone_ref_wrong_kind",
                                message: format!(
                                    "feature `milestone` points at {milestone_id} which is kind = \"{}\", not `milestone` — stale milestone reference",
                                    record.kind.as_str()
                                ),
                            });
                        }
                    }
                }
            }
        }
    }

    // Rollup-derived milestone findings: the second consumer of
    // `mmcp_store::milestones::rollup`, alongside
    // `milestones::list_milestones`.
    // Best-effort against the process-global cache pool: the cache
    // is a derived artifact, so its absence (e.g. a consumer that
    // never called `cache::init_from_home`) is never a reason to
    // fail the rest of `diagnose`.
    if let Some(pool) = crate::cache::active_pool() {
        for (entry, corpus) in entries.iter().zip(&corpora) {
            let gid = entry.handle.group_id.to_string();
            for (file_ref, outcome) in corpus {
                let Ok(fm) = outcome else {
                    continue;
                };
                if fm.kind != MemoryKind::Milestone {
                    continue;
                }
                let Some(meta) = fm.milestone.as_ref() else {
                    continue;
                };
                let Some(id) = fm.id else {
                    continue;
                };
                let Ok(computed) = crate::milestones::rollup::compute(
                    &pool,
                    backend,
                    groups,
                    entry.handle.group_id,
                    id,
                )
                .await
                else {
                    continue;
                };
                let Some(report) = reports.iter_mut().find(|r| r.group_id == gid) else {
                    continue;
                };
                if computed.counted == 0 {
                    report.findings.push(Finding {
                        group: gid.clone(),
                        slug: Some(file_ref.slug.clone()),
                        severity: "info",
                        code: "milestone_rollup_empty",
                        message: "no locally-mirrored feature currently targets this milestone"
                            .to_string(),
                    });
                }
                let editorial_completed =
                    meta.status == mmcp_core::memory::MilestoneStatus::Completed;
                let rollup_completed =
                    computed.status == crate::milestones::rollup::RollupStatus::Completed;
                if editorial_completed != rollup_completed {
                    report.findings.push(Finding {
                        group: gid.clone(),
                        slug: Some(file_ref.slug.clone()),
                        severity: "warning",
                        code: "milestone_status_stale",
                        message: format!(
                            "milestone status is '{}' but the live rollup over its features is '{}' — stale milestone status",
                            meta.status.as_str(),
                            computed.status.as_str()
                        ),
                    });
                }
            }
        }
    }

    DiagReport {
        project_findings,
        groups: reports,
    }
}

/// Check project-level and user-level configuration for findings.
fn check_project_config(findings: &mut Vec<Finding>) {
    // User-level config
    let home = MmcpHome::discover().ok();
    let user_cfg = home
        .as_ref()
        .and_then(|h| h.load_user_config().ok())
        .unwrap_or_default();

    if home.as_ref().is_none_or(|h| !h.user_config_path().exists()) {
        findings.push(Finding {
            group: "(user)".to_string(),
            slug: None,
            severity: "warning",
            code: "user_config_missing",
            message: "no user config at ~/.mmcp/config.toml - create one to set author identity and default sync server".to_string(),
        });
    } else {
        // Check author config
        match user_cfg.author.as_ref() {
            None => {
                findings.push(Finding {
                    group: "(user)".to_string(),
                    slug: None,
                    severity: "warning",
                    code: "user_author_missing",
                    message: "[author] section missing in user config".to_string(),
                });
            }
            Some(author) => {
                match author.git_fallback {
                    None => {
                        findings.push(Finding {
                            group: "(user)".to_string(),
                            slug: None,
                            severity: "warning",
                            code: "user_author_git_fallback_unset",
                            message: "author.git_fallback not set - set to true (use git identity) or false (use mmcp fallback)".to_string(),
                        });
                    }
                    Some(true) => {
                        // Check if git config actually has a name when no override is set.
                        // Uses the same gix-based reader as author resolution so the two
                        // views can never disagree.
                        if author.name.is_none() && read_git_global("user.name").is_none() {
                            findings.push(Finding {
                                group: "(user)".to_string(),
                                slug: None,
                                severity: "warning",
                                code: "user_author_git_name_empty",
                                message: "git_fallback=true but git config user.name is empty"
                                    .to_string(),
                            });
                        }
                    }
                    Some(false) => {
                        // Explicit opt-out: respected silently, no warning
                    }
                }
            }
        }
    }

    // Check sync: user config OR project config must have it
    let user_has_sync = !user_cfg.sync.is_empty();

    // Project-level config
    let cwd = match std::env::current_dir() {
        Ok(d) => d,
        Err(_) => {
            if !user_has_sync {
                findings.push(Finding {
                    group: "(project)".to_string(),
                    slug: None,
                    severity: "warning",
                    code: "sync_not_configured",
                    message: "no [sync] server configured in user or project config".to_string(),
                });
            }
            return;
        }
    };

    match crate::config::find_project_root(&cwd) {
        None => {
            findings.push(Finding {
                group: "(project)".to_string(),
                slug: None,
                severity: "warning",
                code: "project_config_missing",
                message: "no .mmcp.toml project config found in current directory or any parent"
                    .to_string(),
            });
            if !user_has_sync {
                findings.push(Finding {
                    group: "(project)".to_string(),
                    slug: None,
                    severity: "warning",
                    code: "sync_not_configured",
                    message: "no [sync] server configured anywhere - push/pull will not work"
                        .to_string(),
                });
            }
        }
        Some(root) => match crate::config::load(&root) {
            Err(err) => {
                findings.push(Finding {
                    group: "(project)".to_string(),
                    slug: None,
                    severity: "error",
                    code: "project_config_load_failed",
                    message: format!("project config failed to load: {err}"),
                });
            }
            Ok(cfg) => {
                if cfg.sync.is_empty() && !user_has_sync {
                    findings.push(Finding {
                        group: "(project)".to_string(),
                        slug: None,
                        severity: "warning",
                        code: "sync_not_configured",
                        message: "no [sync] server configured in user or project config - push/pull will not work".to_string(),
                    });
                }
            }
        },
    }
}

/// Return true when two slug-shaped strings share at least one substantive token.
/// "Substantive" = length ≥ 3 and not one of a short inline stopword list (no dep needed).
/// Used to gate the slug/name drift heuristic
/// so curated-short-label vs. full-title pairs (e.g. `global-coding-rules-rust` vs. `rust-coding-rules`)
/// don't trip the check.
fn tokens_overlap(a: &str, b: &str) -> bool {
    const STOPWORDS: &[&str] = &[
        "the", "and", "for", "with", "from", "into", "that", "this", "but", "not", "are", "you",
        "have", "has", "was", "were", "will",
    ];
    let tokenize = |s: &str| -> Vec<String> {
        s.split('-')
            .filter(|t| t.len() >= 3 && !STOPWORDS.contains(t))
            .map(|t| t.to_string())
            .collect()
    };
    let at = tokenize(a);
    let bt = tokenize(b);
    at.iter().any(|t| bt.iter().any(|u| u == t))
}

#[cfg(test)]
mod frontmatter_corpus_tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    /// File count large enough that a per-file `read_file` loop and a
    /// single batched call are trivially distinguishable.
    const BULK_FILE_COUNT: usize = 12;

    fn bulk_files() -> Vec<MemoryFileRef> {
        (0..BULK_FILE_COUNT)
            .map(|i| MemoryFileRef {
                slug: format!("bulk-{i}"),
                id: Uuid::now_v7(),
                path: format!("memories/bulk-{i}/dummy.md"),
            })
            .collect()
    }

    /// A minimal, well-formed memory file body so `parse_frontmatter`
    /// succeeds for every seeded file.
    fn sample_memory_body(id: Uuid) -> String {
        format!(
            "+++\nid = \"{id}\"\nname = \"n\"\ndescription = \"d\"\nkind = \"rule\"\n+++\nbody\n"
        )
    }

    /// [`build_frontmatter_corpus`] calls its `read_batch` seam exactly
    /// once, independent of file count, and the resulting corpus is then
    /// consumed by simulated stand-ins for all four of `diagnose_all`'s
    /// frontmatter-only passes (slug-dup, by-id, cross-ref,
    /// milestone-rollup) without the seam firing again. The regression
    /// this guards is each pass running its own `read_file`-per-path
    /// loop, which called the seam once per file per pass instead of
    /// once for the whole group.
    #[tokio::test]
    async fn build_frontmatter_corpus_reads_the_batch_exactly_once_across_all_four_passes() {
        let files = bulk_files();
        let requested_len = files.len();
        let call_count = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&call_count);

        let corpus = build_frontmatter_corpus(files, move |batched_paths| {
            counted.fetch_add(1, Ordering::SeqCst);
            assert_eq!(
                batched_paths.len(),
                requested_len,
                "every file must land in the single batch call"
            );
            async move {
                let batch = batched_paths
                    .into_iter()
                    .map(|path| {
                        let id = Uuid::now_v7();
                        (path, Ok(bytes::Bytes::from(sample_memory_body(id))))
                    })
                    .collect();
                Ok(batch)
            }
        })
        .await;

        // Stand in for the four downstream passes, each walking the same
        // corpus independently the way `diagnose_all` does.
        let slug_dup_pass = corpus.len();
        let by_id_pass = corpus.iter().filter(|(_, o)| o.is_ok()).count();
        let cross_ref_pass = corpus.iter().filter(|(_, o)| o.is_ok()).count();
        let milestone_pass = corpus.iter().filter(|(_, o)| o.is_ok()).count();

        assert_eq!(slug_dup_pass, BULK_FILE_COUNT);
        assert_eq!(by_id_pass, BULK_FILE_COUNT);
        assert_eq!(cross_ref_pass, BULK_FILE_COUNT);
        assert_eq!(milestone_pass, BULK_FILE_COUNT);
        assert_eq!(
            call_count.load(Ordering::SeqCst),
            1,
            "the read seam must be called exactly once no matter how many passes consume the corpus"
        );
    }

    /// A batch-level failure (revision does not resolve) fails every file
    /// in the group identically, matching the pre-refactor per-file loop
    /// where the same failure hit every individual `read_file` call.
    #[tokio::test]
    async fn build_frontmatter_corpus_batch_failure_fails_every_file() {
        let files = bulk_files();

        let corpus = build_frontmatter_corpus(files, |_paths| async {
            Err(GitError::RevNotFound("HEAD".to_string()))
        })
        .await;

        assert_eq!(corpus.len(), BULK_FILE_COUNT);
        assert!(
            corpus.iter().all(|(_, outcome)| outcome.is_err()),
            "every file must be marked unread when the batch call itself fails"
        );
    }
}

#[cfg(test)]
mod milestone_reference_tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use crate::testing::ScratchHome;

    #[tokio::test]
    async fn dangling_milestone_reference_is_flagged() {
        let scratch = ScratchHome::new().await.expect("scratch home");
        let seeded = scratch
            .seed_group("diag-milestone-group")
            .await
            .expect("seed");
        let entry = scratch.groups().get(&seeded.group_id).await.expect("entry");

        crate::features::add_feature(
            scratch.backend(),
            &entry,
            crate::features::AddSpec {
                slug: Some("dangling-feat".into()),
                title: "feat".into(),
                description: "dangling milestone ref test".into(),
                body: "x".into(),
                milestone: Some(Uuid::now_v7()),
                ..crate::features::AddSpec::default()
            },
            scratch.author(),
        )
        .await
        .expect("seed feature");

        let report = diagnose_all(scratch.backend(), scratch.groups()).await;
        let group_report = report
            .groups
            .iter()
            .find(|r| r.group_id == entry.handle.group_id.to_string())
            .expect("group report present");
        assert!(
            group_report
                .findings
                .iter()
                .any(|f| f.code == "milestone_ref_dangling"),
            "findings: {:?}",
            group_report.findings
        );
    }

    #[tokio::test]
    async fn milestone_reference_pointing_at_wrong_kind_is_flagged() {
        let scratch = ScratchHome::new().await.expect("scratch home");
        let seeded = scratch
            .seed_group("diag-milestone-group")
            .await
            .expect("seed");
        let entry = scratch.groups().get(&seeded.group_id).await.expect("entry");

        crate::features::add_feature(
            scratch.backend(),
            &entry,
            crate::features::AddSpec {
                slug: Some("target-feat".into()),
                title: "target feat".into(),
                description: "wrong kind target".into(),
                body: "x".into(),
                ..crate::features::AddSpec::default()
            },
            scratch.author(),
        )
        .await
        .expect("seed target feature");
        let target_id = crate::memory::resolve_memory(
            scratch.backend(),
            &entry.handle,
            Some("target-feat"),
            None,
        )
        .await
        .expect("resolve target")
        .id;

        crate::features::add_feature(
            scratch.backend(),
            &entry,
            crate::features::AddSpec {
                slug: Some("pointing-feat".into()),
                title: "pointing feat".into(),
                description: "stale milestone ref test".into(),
                body: "x".into(),
                milestone: Some(target_id),
                ..crate::features::AddSpec::default()
            },
            scratch.author(),
        )
        .await
        .expect("seed pointing feature");

        let report = diagnose_all(scratch.backend(), scratch.groups()).await;
        let group_report = report
            .groups
            .iter()
            .find(|r| r.group_id == entry.handle.group_id.to_string())
            .expect("group report present");
        assert!(
            group_report
                .findings
                .iter()
                .any(|f| f.code == "milestone_ref_wrong_kind"),
            "findings: {:?}",
            group_report.findings
        );
    }

    #[tokio::test]
    async fn milestone_with_no_dangling_ref_stays_clean() {
        let scratch = ScratchHome::new().await.expect("scratch home");
        let seeded = scratch
            .seed_group("diag-milestone-group")
            .await
            .expect("seed");
        let entry = scratch.groups().get(&seeded.group_id).await.expect("entry");

        let milestone = crate::milestones::add_milestone(
            scratch.backend(),
            &entry,
            crate::milestones::AddSpec {
                slug: Some("clean-milestone".into()),
                title: "Clean".into(),
                ..crate::milestones::AddSpec::default()
            },
            scratch.author(),
        )
        .await
        .expect("seed milestone");
        let milestone_id = crate::memory::resolve_memory(
            scratch.backend(),
            &entry.handle,
            Some("clean-milestone"),
            None,
        )
        .await
        .expect("resolve milestone")
        .id;

        crate::features::add_feature(
            scratch.backend(),
            &entry,
            crate::features::AddSpec {
                slug: Some("clean-feat".into()),
                title: "clean feat".into(),
                description: "valid milestone ref".into(),
                body: "x".into(),
                milestone: Some(milestone_id),
                ..crate::features::AddSpec::default()
            },
            scratch.author(),
        )
        .await
        .expect("seed feature");

        let report = diagnose_all(scratch.backend(), scratch.groups()).await;
        let group_report = report
            .groups
            .iter()
            .find(|r| r.group_id == entry.handle.group_id.to_string())
            .expect("group report present");
        assert!(
            !group_report
                .findings
                .iter()
                .any(|f| f.code == "milestone_ref_dangling" || f.code == "milestone_ref_wrong_kind"),
            "a valid milestone reference must not be flagged: {:?}",
            group_report.findings
        );
        let _ = milestone;
    }
}
