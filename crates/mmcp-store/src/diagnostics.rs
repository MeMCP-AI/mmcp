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
use mmcp_core::memory::{MemoryFile, MemoryKind, parse_sections};
use mmcp_git::{GitBackend, NativeBackend, Rev};
use serde::Serialize;
use uuid::Uuid;

use crate::groups::{GroupEntry, GroupIndex};
use crate::home::{MmcpHome, read_git_global};
use crate::memory::slugify_filename;

// ── Shared types ────────────────────────────────────────────

/// One finding emitted by a check.
/// `code` is a stable slug-style identifier (e.g. `manifest_unreadable`, `memory_body_empty`),
/// that lets consumers branch without parsing the free-form `message`.
/// The MCP tool boundary maps each finding onto a [`mmcp_proto::Note`] using this code.
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

/// Quick surface check: manifest parseable, all memories parse,
/// required fields present. No hints, no deep analysis.
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
                let Ok(text) = std::str::from_utf8(&bytes) else {
                    findings.push(Finding {
                        group: gid.clone(),
                        slug: Some(mem_slug.to_string()),
                        severity: "error",
                        code: "memory_not_utf8",
                        message: "not valid UTF-8".to_string(),
                    });
                    continue;
                };
                if let Err(err) = MemoryFile::parse(text) {
                    findings.push(Finding {
                        group: gid.clone(),
                        slug: Some(mem_slug.to_string()),
                        severity: "error",
                        code: "frontmatter_parse_failed",
                        message: format!("frontmatter parse failed: {err}"),
                    });
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

/// Deep diagnostic analysis. Runs everything health does plus:
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

    // Track feature numbers seen within this group so we can flag
    // duplicates after the per-memory loop. `add_feature` enforces
    // monotonic uniqueness on writes, but a manual edit could
    // reintroduce a collision.
    let mut feature_numbers: std::collections::HashMap<u32, Vec<String>> =
        std::collections::HashMap::new();

    // Index every feature in this group by its UUID so the
    // post-loop supersede-chain integrity pass can look targets up
    // without a second pass over the files. Value is
    // `(slug, status, refs)`, enough to verify reciprocity
    // without carrying the full frontmatter around.
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

        // Slug/name drift: fire only when the slug and slugified
        // name share *no* meaningful tokens. Substring containment
        // is too loose: a short curated slug ("global-coding-
        // rules-rust") and a full title ("Rust Coding Rules")
        // legitimately differ as abbreviation vs. expansion and
        // shouldn't spam the report. Zero-overlap is a strong
        // signal the name actually changed without the slug
        // rotating.
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

        // Every `kind = "feature"` memory should carry a
        // sequential `number`.
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

        // Kind vs `[feature]` subtable consistency. A `feature`
        // kind must carry a `[feature]` block; no other kind may.
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

    // Supersede-chain integrity: every feature with a typed
    // `superseded_by` back-link should have its target present in
    // the same group, reciprocating via a `refs` entry pointing
    // back at the source's UUID. Catches the half-landed
    // two-commit supersede case: commit A wrote the new feature,
    // commit B never flipped the old one's status, or the reverse,
    // commit B flipped it but an operator later removed the new
    // feature's ref by hand.
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

    // Walk every leaf slug dir so stray non-UUID filenames are
    // surfaced. Nested slug paths surface as separate leaf entries;
    // intermediate path nodes are not "empty leaves".
    // `list_all_memory_files` silently skips non-UUID files, so
    // without this pass a `memories/rules/scratch.md` would never
    // show up in diagnostics.
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

/// Human-readable name of a [`GroupScope`] for diagnostic
/// messages. The serde rename_all derives the same lowercase
/// tokens we want to quote back at operators.
fn scope_str(scope: GroupScope) -> &'static str {
    match scope {
        GroupScope::Global => "global",
        GroupScope::Shared => "shared",
        GroupScope::Project => "project",
    }
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

    for entry in &entries {
        let report = diagnose_group(backend, entry).await;
        let gid = report.group_id.clone();
        let rev = Rev::head();
        if let Ok(files) = crate::memory::list_all_memory_files(backend, &entry.handle, &rev).await
        {
            // Dedupe per-group: under the two-level layout a slug
            // with multiple UUIDs is still one slug from the
            // cross-group-duplicate perspective.
            let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
            for file in files {
                if seen.insert(file.slug.clone()) {
                    slug_groups.entry(file.slug).or_default().push(gid.clone());
                }
            }
        }
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

    // Build a single registry of every memory across the mirror so
    // downstream cross-ref validation and duplicate-UUID detection
    // share one pass over the repos. Each entry is keyed by UUID;
    // the value carries enough context to point operators at the
    // offending file when a collision fires.
    #[derive(Clone)]
    struct MemoryRecord {
        group_id: String,
        slug: String,
        kind: MemoryKind,
    }
    let mut by_id: std::collections::HashMap<Uuid, Vec<MemoryRecord>> =
        std::collections::HashMap::new();
    for entry in &entries {
        let gid = entry.handle.group_id.to_string();
        let rev = Rev::head();
        let Ok(files) = crate::memory::list_all_memory_files(backend, &entry.handle, &rev).await
        else {
            continue;
        };
        for file_ref in files {
            let Ok(bytes) = backend.read_file(&entry.handle, &file_ref.path, &rev).await else {
                continue;
            };
            let Ok(text) = std::str::from_utf8(&bytes) else {
                continue;
            };
            let Ok(mf) = MemoryFile::parse(text) else {
                continue;
            };
            by_id.entry(file_ref.id).or_default().push(MemoryRecord {
                group_id: gid.clone(),
                slug: file_ref.slug.clone(),
                kind: mf.frontmatter.kind,
            });
        }
    }

    // Duplicate UUIDs across memories. Every memory's UUID is its
    // primary key; two memories sharing one breaks `resolve_by_id`
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
    for entry in &entries {
        let rev = Rev::head();
        let gid = entry.handle.group_id.to_string();
        let Ok(files) = crate::memory::list_all_memory_files(backend, &entry.handle, &rev).await
        else {
            continue;
        };
        for file_ref in files {
            let Ok(bytes) = backend.read_file(&entry.handle, &file_ref.path, &rev).await else {
                continue;
            };
            let Ok(text) = std::str::from_utf8(&bytes) else {
                continue;
            };
            let Ok(mf) = MemoryFile::parse(text) else {
                continue;
            };
            if mf.frontmatter.kind != MemoryKind::Feature {
                continue;
            }
            let Some(feat) = mf.frontmatter.feature else {
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
        for entry in &entries {
            let rev = Rev::head();
            let gid = entry.handle.group_id.to_string();
            let Ok(files) =
                crate::memory::list_all_memory_files(backend, &entry.handle, &rev).await
            else {
                continue;
            };
            for file_ref in files {
                let Ok(bytes) = backend.read_file(&entry.handle, &file_ref.path, &rev).await else {
                    continue;
                };
                let Ok(text) = std::str::from_utf8(&bytes) else {
                    continue;
                };
                let Ok(mf) = MemoryFile::parse(text) else {
                    continue;
                };
                if mf.frontmatter.kind != MemoryKind::Milestone {
                    continue;
                }
                let Some(meta) = mf.frontmatter.milestone else {
                    continue;
                };
                let Some(id) = mf.frontmatter.id else {
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
    let user_has_sync = user_cfg.sync.is_some();

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
                if cfg.sync.is_none() && !user_has_sync {
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

/// Return true when two slug-shaped strings share at least one
/// substantive token. "Substantive" = length ≥ 3 and not one of
/// a short stopword list we keep inline (no dep needed). Used to
/// gate the slug/name drift heuristic so curated-short-label vs.
/// full-title pairs (e.g. `global-coding-rules-rust` vs.
/// `rust-coding-rules`) don't trip the check.
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
mod milestone_reference_tests {
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
